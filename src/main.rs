#![forbid(clippy::unwrap_used)]
#![forbid(clippy::expect_used)]

use log::{error, info, warn};
use redox_log::{OutputBuilder, RedoxLogger};
use std::{env, process::Command, rc::Rc};

use config::Config;
use core::Orbital;

mod compositor;
mod config;
mod core;
mod scheme;
mod widget;
mod window;
mod window_order;

/// Run orbital main event loop, starting a login command before entering the event loop.
fn orbital() -> Result<(), String> {
    // Ignore possible errors while enabling logging
    let _ = RedoxLogger::new()
        .with_output(
            OutputBuilder::stdout()
                .with_filter(log::LevelFilter::Warn)
                .with_ansi_escape_codes()
                .build(),
        )
        .with_process_name("orbital".into())
        .enable();

    let mut args = env::args().skip(1);
    let vt = env::var("VT").expect("`VT` environment variable not set");
    unsafe {
        env::remove_var("VT");
    }
    let (scheme_name, login_cmd) = match args.next().ok_or("no login manager argument")? {
        arg if arg == "--scheme" => {
            let scheme_name = args.next().ok_or("no scheme name argument")?;
            let login_cmd = args.next().ok_or("no login manager argument")?;
            (scheme_name, login_cmd)
        }
        login_cmd => ("orbital".to_owned(), login_cmd),
    };

    let config = Rc::new(Config::from_path("/ui/orbital.toml"));

    let orbital = Orbital::open_display(config)
        .map_err(|e| format!("could not open display, caused by: {}", e))?;

    match Command::new("inputd").arg("-A").arg(&vt).status() {
        Ok(status) => {
            if !status.success() {
                warn!("inputd -A '{}' exited with status: {:?}", vt, status);
            }
        }
        Err(err) => {
            warn!("inputd -A '{}' failed to run with error: {}", vt, err);
        }
    }

    orbital
        .run(&scheme_name, Command::new(login_cmd).args(args))
        .map_err(|e| format!("error in main loop, caused by {}", e))
}

/// Start orbital. This will start orbital main event loop.
///
/// Startup messages and errors are logged to RedoxLogger with filter set to DEBUG
fn main() {
    redox_log::RedoxLogger::init_timezone();
    match orbital() {
        Ok(()) => {
            info!("ran to completion successfully, exiting with status=0");
            std::process::exit(0);
        }
        Err(e) => {
            error!("error during daemon execution, exiting with status=1: {e}");
            std::process::exit(1);
        }
    }
}
