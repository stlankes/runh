use command_fds::{CommandFdExt, FdMapping};
use nix::fcntl::OFlag;
use nix::sys::socket;
use nix::sys::socket::SockFlag;
use nix::sys::stat::Mode;
use nix::unistd::Gid;
use nix::unistd::Uid;
use oci_spec::runtime;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::prelude::*;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::prelude::CommandExt;
use std::os::unix::prelude::FromRawFd;
use std::os::unix::prelude::IntoRawFd;
use std::path::PathBuf;
use std::str::FromStr;

use crate::container::OCIContainer;

pub fn create_container(
	project_dir: PathBuf,
	id: Option<&str>,
	bundle: Option<&str>,
	pidfile: Option<&str>,
	console_socket: Option<&str>,
) {
	let mut path = project_dir.clone();

	let _ = std::fs::create_dir(path.clone());

	path.push(id.unwrap());
	std::fs::create_dir(path.clone()).expect("Unable to create container directory");
	let container = OCIContainer::new(
		bundle.unwrap().to_string(),
		id.unwrap().to_string(),
		pidfile
			.unwrap_or(&(path.to_str().unwrap().to_owned() + "/containerpid"))
			.to_string(),
	);

	// write container to disk
	let spec_path = path.join("container.json");
	let mut file = OpenOptions::new()
		.read(true)
		.write(true)
		.create_new(true)
		.open(&spec_path)
		.expect("Unable to create container");
	file.write_all(serde_json::to_string(&container).unwrap().as_bytes())
		.unwrap();

	debug!(
		"Create container with uid {}, gid {}",
		container.spec().process.as_ref().unwrap().user.uid,
		container.spec().process.as_ref().unwrap().user.gid
	);

	//Setup exec fifo
	let fifo_location = path.join("exec.fifo");
	let old_mask = Mode::from_bits_truncate(0o000);
	nix::unistd::mkfifo(&fifo_location, Mode::from_bits_truncate(0o644))
		.expect("Could not create fifo!");

	let _ = nix::sys::stat::umask(old_mask);
	nix::unistd::chown(
		&fifo_location,
		Some(Uid::from_raw(0)),
		Some(Gid::from_raw(0)),
	)
	.expect("could not call chown!");

	let fifo = OpenOptions::new()
		.custom_flags(libc::O_PATH | libc::O_CLOEXEC)
		.read(true)
		.write(false)
		.mode(0)
		.open(&fifo_location)
		.expect("Could not open fifo!");

	//Setup init pipe
	let (parent_socket_fd, child_socket_fd) = socket::socketpair(
		socket::AddressFamily::Unix,
		socket::SockType::Stream,
		None,
		SockFlag::SOCK_CLOEXEC,
	)
	.expect("Could not create socket pair for init pipe!");

	//Setup log pipe
	let (parent_log_fd, child_log_fd) =
		nix::unistd::pipe2(OFlag::O_CLOEXEC).expect("Could not create socket pair for log pipe!");
	let log_forwarder = std::thread::spawn(move || {
		let log_pipe = unsafe { std::fs::File::from_raw_fd(parent_log_fd) };
		let mut reader = std::io::BufReader::new(log_pipe);
		let mut buffer: Vec<u8> = vec![];
		while let Ok(bytes_read) = reader.read_until(b"}"[0], &mut buffer) {
			if bytes_read > 0 {
				if let Ok(log_entry) =
					serde_json::from_slice::<crate::logging::LogEntry>(buffer.as_slice())
				{
					match log::Level::from_str(log_entry.level.as_str()) {
						Ok(level) => log!(level, "[INIT] {}", log_entry.msg),
						Err(_) => info!("[INIT] {}", log_entry.msg),
					}
					buffer.clear();
				}
			} else {
				debug!("Read zero bytes from log pipe, closing forwarder...");
				break;
			}
		}
	});

	//Pass spec file
	let mut config = std::path::PathBuf::from(bundle.unwrap().to_string());
	config.push("config.json");
	let spec_file = File::open(config).expect("Could not open spec file!");

	let mut child_fd_mappings = vec![
		FdMapping {
			parent_fd: fifo.as_raw_fd(),
			child_fd: 3,
		},
		FdMapping {
			parent_fd: child_socket_fd,
			child_fd: 4,
		},
		FdMapping {
			parent_fd: spec_file.as_raw_fd(),
			child_fd: 5,
		},
		FdMapping {
			parent_fd: child_log_fd,
			child_fd: 6,
		},
	];

	//Setup console socket
	let socket_fds = if let Some(console_socket_path) = console_socket {
		let stream = UnixStream::connect(PathBuf::from(console_socket_path)).expect(
			format!(
				"Could not connect to socket named by console-socket path at {}",
				console_socket_path
			)
			.as_str(),
		);
		let sock_stream_fd = stream.into_raw_fd();
		let socket_fd_copy =
			nix::unistd::dup(sock_stream_fd).expect("Could not duplicate unix stream fd!");
		child_fd_mappings.push(FdMapping {
			parent_fd: socket_fd_copy,
			child_fd: 7,
		});
		Some((socket_fd_copy, sock_stream_fd))
	} else {
		None
	};

	let _ = std::process::Command::new("/proc/self/exe")
		.arg("-l")
		.arg("debug") //TODO: Start child process with the same log level the parent was called with
		.arg("--log-format")
		.arg("json")
		.arg("init")
		.fd_mappings(child_fd_mappings)
		.expect("Unable to pass fifo fd to child!")
		.env("RUNH_FIFOFD", "3")
		.env("RUNH_INITPIPE", "4")
		.env("RUNH_SPEC_FILE", "5")
		.env("RUNH_LOG_PIPE", "6")
		.env("RUNH_CONSOLE", "7")
		.spawn()
		.expect("Unable to spawn runh init process");

	debug!("Started init process. Closing child fds in create process.");
	nix::unistd::close(child_socket_fd).expect("Could not close child_socket_fd!");
	nix::unistd::close(child_log_fd).expect("Could not close child_log_fd!");
	if let Some((socket_fd, stream_fd)) = socket_fds {
		nix::unistd::close(socket_fd).expect("Could not close console socket_fd!");
		nix::unistd::close(stream_fd).expect("Could not close console stream_fd!");
	}

	debug!("Waiting for first message from child...");
	let mut init_pipe = unsafe { File::from_raw_fd(parent_socket_fd) };
	let mut buffer: [u8; 1] = [1];
	init_pipe
		.read_exact(&mut buffer)
		.expect("Could not read from init pipe!");
	debug!("Read from init pipe: {}", buffer[0]);

	let rootfs_path = PathBuf::from(&container.spec().root.as_ref().unwrap().path);
	let rootfs_path_abs = if rootfs_path.is_absolute() {
		rootfs_path
	} else {
		PathBuf::from(bundle.unwrap()).join(rootfs_path)
	};
	let rootfs_path_str = std::fs::canonicalize(rootfs_path_abs)
		.expect("Could not parse path to rootfs!")
		.as_os_str()
		.to_str()
		.expect("Could not convert rootfs-path to string!")
		.to_string();
	debug!(
		"Write rootfs-path {} (lenght {}) to init-pipe!",
		rootfs_path_str,
		rootfs_path_str.len()
	);
	init_pipe
		.write(&(rootfs_path_str.len() as usize).to_le_bytes())
		.expect("Could not write rootfs-path size to init pipe!");

	init_pipe
		.write_all(rootfs_path_str.as_bytes())
		.expect("Could not write rootfs-path to init pipe!");

	debug!("Waiting for runh init to send grandchild PID");
	let mut pid_buffer = [0; 4];
	init_pipe
		.read_exact(&mut pid_buffer)
		.expect("Could not read from init pipe!");

	let pid = i32::from_le_bytes(pid_buffer);
	if let Some(pid_file_path) = pidfile {
		let mut file = std::fs::File::create(pid_file_path).expect("Could not create pid-File!");
		write!(file, "{}", pid).expect("Could not write to pid-file!");
	}
	let mut sig_buffer = [0u8];

	debug!("Waiting for runh init to request prestart hooks");

	init_pipe
		.read_exact(&mut sig_buffer)
		.expect("Could not read from init pipe!");
	if sig_buffer[0] != crate::consts::INIT_REQ_PRESTART_HOOKS {
		panic!(
			"Received invalid signal from runh init! Expected {:x}, got {:x}",
			crate::consts::INIT_REQ_PRESTART_HOOKS,
			sig_buffer[0]
		);
	}

	debug!("Running prestart hooks...");
	if let Some(hooks) = container.spec().hooks.as_ref() {
		let state = runtime::State {
			version: String::from("1.0.2"),
			id: container.id().clone(),
			status: String::from("created"),
			pid: Some(pid),
			bundle: container.bundle().clone(),
			annotations: container.spec().annotations.clone(),
		};

		if let Some(prestart_hooks) = hooks.prestart.as_ref() {
			for hook in prestart_hooks {
				let mut cmd = std::process::Command::new(&hook.path);
				if let Some(args) = &hook.args {
					if !args.is_empty() {
						cmd.arg0(&args[0]);
					}
					if args.len() > 1 {
						cmd.args(&args[1..]);
					}
				}
				if let Some(env) = &hook.env {
					for var in env {
						let (name, value) = var.split_once("=").expect(
							format!("Could not parse environment variable: {}", var).as_str(),
						);
						cmd.env(name, value);
					}
				}
				if let Some(timeout) = hook.timeout {
					if timeout <= 0 {
						error!("prestart hook {} has a timeout <= 0!", hook.path);
					} else {
						warn!("The timeout set for prestart hook {} is currently unimplemented and will be ignored!", hook.path);
					}
				}
				cmd.stderr(std::process::Stdio::piped());
				cmd.stdin(std::process::Stdio::piped());
				let mut child = cmd.spawn().expect(
					format!("Unable to spawn prestart hook process {}", hook.path).as_str(),
				);
				write!(
					child.stdin.take().unwrap(),
					"{}",
					state.to_string().unwrap()
				)
				.expect("Could not write container state to hook process stdin!");

				let ret = child.wait_with_output().unwrap();
				if !ret.status.success() {
					panic!(
						"prestart hook {} returned exit status {}. Stderr: {}",
						hook.path,
						ret.status,
						String::from_utf8(ret.stderr).unwrap()
					);
				}
			}
		}
	}

	init_pipe
		.write(&[crate::consts::CREATE_ACK_PRESTART_HOOKS])
		.expect("Unable to write to init-pipe!");

	debug!("Waiting for runh init to get ready to execv!");

	if let Err(x) = init_pipe.read_exact(&mut sig_buffer) {
		log_forwarder.join().expect("Log forwarder did panic!");
		panic!("Could not read from init-pipe! Init probably died: {}", x);
	} else {
		if sig_buffer[0] == crate::consts::INIT_READY_TO_EXECV {
			info!("Runh init ran successfully and is now ready to execv. Waiting for log pipe to close...");
		} else {
			panic!("Received invalid signal from runh init!");
		}
	}
	return;
}
