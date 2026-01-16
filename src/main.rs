#![forbid(clippy::unwrap_used)]
#![forbid(clippy::expect_used)]

use crate::core::Orbital;
use log::{debug, error, info, warn};
use redox_log::{OutputBuilder, RedoxLogger};
use std::{env, process::Command, rc::Rc};

use config::Config;
use scheme::OrbitalScheme;

mod compositor;
mod config;
mod core;
mod window_order;
mod scheme;
mod window;

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
    let login_cmd = args.next().ok_or("no login manager argument")?;

    let (orbital, displays) = Orbital::open_display(&vt)
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

    debug!(
        "found display {}x{}",
        displays[0].image.width(),
        displays[0].image.height()
    );
    let config = Rc::new(Config::from_path("/ui/orbital.toml"));
    let scheme = OrbitalScheme::new(displays, config)?;

    Command::new(login_cmd)
        .args(args)
        .spawn()
        .map_err(|_| "failed to spawn login_cmd")?;

    orbital
        .run(scheme)
        .map_err(|e| format!("error in main loop, caused by {}", e))
}

/// Start orbital. This will start orbital main event loop.
///
/// Startup messages and errors are logged to RedoxLogger with filter set to DEBUG
fn main() {
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
