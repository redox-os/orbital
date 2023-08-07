#![forbid(clippy::unwrap_used)]
#![forbid(clippy::expect_used)]
#![feature(int_roundings)]

use crate::core::Orbital;
use std::{
    env,
    process::Command,
    rc::Rc
};
use redox_log::{OutputBuilder, RedoxLogger};
use log::{debug, error, info, warn};
use redox_daemon::Daemon;

use config::Config;
use scheme::OrbitalScheme;

mod core;
mod config;
mod scheme;
mod window;

/// Status codes used by this executable on exit
enum OrbitalStatusCode {
    /// main() was able to start the [Daemon][redox_daemon::Daemon] without an error
    Success = 0,
    /// An error occurred when starting the [Daemon][redox_daemon::Daemon]
    EStartingDaemon = 1,
}

/// Status codes used by orbital Daemon on exit
enum DaemonStatusCode {
    /// [Orbital event loop][Orbital::run] ran to completion and ended without error
    Success = 0,
    /// There was a failure during execution in the [Daemon][redox_daemon::Daemon]
    EDaemonFailure= 1,
}

/// Run orbital main event loop in a background daemon, starting a login command before
/// entering the event loop.
///
/// This daemon can fail. If it does so, it will log the failure using error!() logging
/// and exit with a non-zero status. See [DaemonStatusCode]
///
/// This executable (main()) can fail. If it does it will log (error!()) the event and exit with
/// a non-zero status code. See [OrbitalStatusCode]
fn orbital(daemon: Daemon) -> Result<(), String> {
    // Ignore possible errors while enabling logging
    let _ = RedoxLogger::new()
        .with_output(
            OutputBuilder::stdout()
                .with_filter(log::LevelFilter::Debug)
                .with_ansi_escape_codes()
                .build()
        )
        .with_process_name("orbital".into())
        .enable();

    let mut args = env::args().skip(1);
    let vt = env::var("DISPLAY").expect("`DISPLAY` environment variable not set");
    let login_cmd = args.next().ok_or("no login manager argument")?;

    //TODO: integrate this into orbital
    match Command::new("inputd").arg("-G").arg(&vt).status() {
        Ok(status) => if ! status.success() {
            warn!("inputd -G '{}' exited with status: {:?}", vt, status);
        },
        Err(err) => {
            warn!("inputd -G '{}' failed to run with error: {}", vt, err);
        }
    }

    let orbital = Orbital::open_display(&vt)
        .map_err(|e| format!("could not open display, caused by: {}", e))?;
    daemon.ready().unwrap();

    debug!("found display {}x{}", orbital.image().width(), orbital.image().height());
    let config = Rc::new(Config::from_path("/ui/orbital.toml"));
    let scheme = OrbitalScheme::new(
        &orbital.displays,
        config,
    )?;

    Command::new(login_cmd)
        .args(args)
        .spawn()
        .map_err(|_| "failed to spawn login_cmd")?;

    orbital.run(scheme)
        .map_err(|e| format!("error in main loop, caused by {}", e))
}

/// Start orbital. This will start orbital main event loop as a daemon, then exit.
/// Note that the code running in the daemon can also fail and exit with its own non-zero status
/// code at any time after startup.
///
/// Possible status codes on exit are:
/// ORBITAL_SUCCESS 0
/// E_STARTING_DAEMON 1
///
/// Startup messages and errors are logged to RedoxLogger with filter set to DEBUG
pub fn main() {
    match Daemon::new(move |daemon| {
        match orbital(daemon) {
            Ok(_) => {
                info!("ran to completion successfully, exiting with status={}",
                    DaemonStatusCode::Success as i32);
                std::process::exit(DaemonStatusCode::Success as i32);
            },
            Err(e) => {
                error!("error during daemon execution, exiting with status={}: {}",
                    DaemonStatusCode::EDaemonFailure as i32, e);
                std::process::exit(DaemonStatusCode::EDaemonFailure as i32);
            }
        }
    }) {
        Ok(_) => {
            info!("Daemon started, exiting with status={}", OrbitalStatusCode::Success as i32);
            std::process::exit(OrbitalStatusCode::Success as i32);
        },
        Err(e) => {
            error!("error starting daemon, exiting with status={}: {}",
                OrbitalStatusCode::EStartingDaemon as i32, e);
            std::process::exit(OrbitalStatusCode::EStartingDaemon as i32);
        }
    }
}
