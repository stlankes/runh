#[macro_use]
extern crate colour;
#[macro_use]
extern crate log;

mod container;
mod create;
mod cri;
mod delete;
//mod exec;
mod list;
mod logging;
mod pull;
mod run;
mod spec;

use crate::create::*;
use crate::delete::*;
//use crate::exec::*;
use crate::list::*;
use crate::pull::*;
use crate::run::*;
use crate::spec::*;
use clap::{crate_authors, crate_description, crate_version, App, AppSettings, Arg, SubCommand};
use std::str::FromStr;
use std::{env, path::PathBuf};

pub fn get_project_dir() -> PathBuf {
	//let dir = directories::ProjectDirs::from("org", "hermitcore", "runh").expect("Unable to determine container directory");
	//PathBuf::from(dir.project_path().clone())
	PathBuf::from_str("/tmp/runh").unwrap().clone()
}

fn parse_matches(app: App) {
	let matches = app.get_matches();

	// initialize logger
	logging::init(
		matches.value_of("LOG_PATH"),
		matches.value_of("LOG_FORMAT"),
		matches.value_of("LOG_LEVEL"),
	);
	info!("Welcome to runh {}", crate_version!());

	if let Some(ref matches) = matches.subcommand_matches("spec") {
		create_spec(matches.value_of("BUNDLE"));
	} else if let Some(ref matches) = matches.subcommand_matches("create") {
		create_container(
			matches.value_of("CONTAINER_ID"),
			matches.value_of("BUNDLE"),
			matches.value_of("PID_FILE"),
		);
	/* } else if let Some(ref matches) = matches.subcommand_matches("exec") {
	let arguments: Vec<_> = matches
		.values_of("COMMAND OPTIONS")
		.map_or(Vec::new(), |arg| arg.collect());
	exec_command(
		matches.value_of("CONTAINER_ID"),
		matches.value_of("COMMAND"),
		arguments,
	); */
	} else if let Some(ref matches) = matches.subcommand_matches("delete") {
		delete_container(matches.value_of("CONTAINER_ID"));
	} else if let Some(ref matches) = matches.subcommand_matches("run") {
		run_container(matches.value_of("CONTAINER_ID"));
	} else if let Some(_) = matches.subcommand_matches("list") {
		list_containers();
	} else if let Some(ref matches) = matches.subcommand_matches("pull") {
		if let Some(str) = matches.value_of("IMAGE") {
			pull_registry(
				str,
				matches.value_of("USERNAME"),
				matches.value_of("PASSWORD"),
				matches.value_of("BUNDLE"),
			);
		} else {
			error!("Image name is missing");
		}
	} else {
		error!(
			"Subcommand is missing or currently not supported! Run `runh -h` for more information!"
		);
	}
}
pub fn main() {
	std::panic::set_hook(Box::new(|panic_info| {
		error!("PANIC:\n {}", panic_info);
	}));

	let app = App::new("runh")
		.version(crate_version!())
		.setting(AppSettings::ColoredHelp)
		.author(crate_authors!("\n"))
		.about(crate_description!())
		.arg(
			Arg::with_name("ROOT")
				.long("root")
				.takes_value(true)
				.help("root directory for storage of vm state"),
		)
		.arg(
			Arg::with_name("LOG_LEVEL")
				.long("log-level")
				.short("l")
				.default_value("info")
				.possible_values(&["trace", "debug", "info", "warn", "error", "off"])
				.help("The logging level of the application."),
		)
		.arg(
			Arg::with_name("LOG_PATH")
				.long("log")
				.takes_value(true)
				.help("set the log file path"),
		)
		.arg(
			Arg::with_name("LOG_FORMAT")
				.long("log-format")
				.default_value("text")
				.possible_values(&["text", "json"])
				.help("set the log format"),
		)
		.subcommand(
			SubCommand::with_name("spec")
				.about("Create a new specification file")
				.version(crate_version!())
				.arg(
					Arg::with_name("BUNDLE")
						.long("bundle")
						.short("b")
						.required(true)
						.takes_value(true)
						.help("path to the root of the bundle directory"),
				),
		)
		.subcommand(
			SubCommand::with_name("create")
				.about("Create a container")
				.version(crate_version!())
				.arg(
					Arg::with_name("CONTAINER_ID")
						.takes_value(true)
						.required(true)
						.help("Id of the container"),
				)
				.arg(
					Arg::with_name("BUNDLE")
						.long("bundle")
						.short("b")
						.takes_value(true)
						.required(true)
						.help("Path to the root of the bundle directory"),
				)
				.arg(
					Arg::with_name("PID_FILE")
						.long("pid-file")
						.takes_value(true)
						.required(false)
						.help("File to write the process id to"),
				),
		)
		.subcommand(
			SubCommand::with_name("exec")
				.about("Execute new process inside the container")
				.version(crate_version!())
				.arg(
					Arg::with_name("CONTAINER_ID")
						.takes_value(true)
						.required(true)
						.help("Id of the container"),
				)
				.arg(
					Arg::with_name("COMMAND")
						.takes_value(true)
						.required(true)
						.help("Command, which will be executed in the container"),
				)
				.arg(
					Arg::with_name("COMMAND OPTIONS")
						.help("Arguments of the command")
						.required(false)
						.multiple(true)
						.max_values(255),
				),
		)
		.subcommand(
			SubCommand::with_name("delete")
				.about("Delete an existing container")
				.version(crate_version!())
				.arg(
					Arg::with_name("CONTAINER_ID")
						.takes_value(true)
						.required(true)
						.help("Id of the container"),
				),
		)
		.subcommand(
			SubCommand::with_name("list")
				.about("Create and run a container")
				.version(crate_version!()),
		)
		.subcommand(
			SubCommand::with_name("run")
				.about("Create and run a container")
				.version(crate_version!())
				.arg(
					Arg::with_name("CONTAINER_ID")
						.takes_value(true)
						.required(true)
						.help("Id of the container"),
				)
				.arg(
					Arg::with_name("BUNDLE")
						.long("bundle")
						.short("b")
						.takes_value(true)
						.help("Path to the root of the bundle directory"),
				),
		)
		.subcommand(
			SubCommand::with_name("pull")
				.about("Pull an image or a repository from a registry")
				.version(crate_version!())
				.arg(
					Arg::with_name("IMAGE")
						.value_name("NAME[:TAG|@DIGEST]")
						.takes_value(true)
						.help("image or a repository from a registry"),
				)
				.arg(
					Arg::with_name("USERNAME")
						.long("username")
						.short("u")
						.takes_value(true)
						.help("Username"),
				)
				.arg(
					Arg::with_name("PASSWORD")
						.long("password")
						.short("p")
						.takes_value(true)
						.help("Password"),
				)
				.arg(
					Arg::with_name("BUNDLE")
						.long("bundle")
						.short("b")
						.takes_value(true)
						.help("Path to the root of the bundle directory"),
				),
		)
		.subcommand(
			SubCommand::with_name("checkpoint")
				.about("Checkpoint a running container (not supported)")
				.version(crate_version!()),
		)
		.subcommand(
			SubCommand::with_name("restore")
				.about("Restore a container from a previous checkpoint (not supported)")
				.version(crate_version!()),
		);

	parse_matches(app.clone());
}