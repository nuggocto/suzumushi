// SPDX-License-Identifier: Apache-2.0

//! Suzumushi terminal shell, root, configuration, locking, and scanner library.

pub mod app;
pub mod audio;
mod background;
pub mod cli;
pub mod config;
pub mod display;
pub mod errors;
pub mod event;
pub mod init;
pub mod input;
pub mod locks;
pub mod logging;
pub mod metadata;
mod metadata_cache;
pub mod model;
pub mod mpris;
pub mod paths;
pub mod scan;
pub mod state;
pub mod terminal;
pub mod ui;

/// Starts helpers from the running image rather than its installed path.
///
/// A package upgrade replaces the installed file while a session is open, and
/// `std::env::current_exe` then names a deleted path. The kernel keeps this link
/// valid and guarantees the helper speaks the parent's protocol version.
pub(crate) const HELPER_EXECUTABLE: &str = "/proc/self/exe";

/// Binds a helper process to its parent's lifetime and clamps each resource
/// limit to the tighter of the inherited limit and the helper ceiling.
pub(crate) fn apply_helper_limits(
    role: &str,
    limits: &[(rustix::process::Resource, u64)],
) -> Result<(), String> {
    use rustix::process::{
        Pid, Signal, getppid, getrlimit, set_parent_process_death_signal, setrlimit,
    };

    set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|error| format!("cannot bind {role} lifetime to its parent: {error}"))?;
    if getppid().is_none_or(Pid::is_init) {
        return Err(format!("{role} parent exited during helper startup"));
    }
    for &(resource, ceiling) in limits {
        let clamped = clamp_rlimit(getrlimit(resource), ceiling);
        setrlimit(resource, clamped)
            .map_err(|error| format!("cannot apply {role} resource limit: {error}"))?;
    }
    Ok(())
}

pub(crate) fn clamp_rlimit(
    inherited: rustix::process::Rlimit,
    ceiling: u64,
) -> rustix::process::Rlimit {
    let maximum = inherited
        .maximum
        .map_or(ceiling, |limit| limit.min(ceiling));
    let current = inherited
        .current
        .map_or(maximum, |limit| limit.min(ceiling).min(maximum));
    rustix::process::Rlimit {
        current: Some(current),
        maximum: Some(maximum),
    }
}

/// Parses the process command line and returns a conventional exit status.
#[must_use]
pub fn run() -> std::process::ExitCode {
    match cli::run_from(std::env::args_os(), std::env::current_dir) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let message = display::terminal_safe(error.to_string().as_bytes(), 16_384);
            eprintln!("error: {message}");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use rustix::process::Rlimit;

    use super::clamp_rlimit;

    #[test]
    fn helper_rlimits_never_raise_inherited_soft_or_hard_limits() {
        assert_eq!(
            clamp_rlimit(
                Rlimit {
                    current: None,
                    maximum: None,
                },
                600,
            ),
            Rlimit {
                current: Some(600),
                maximum: Some(600),
            }
        );
        assert_eq!(
            clamp_rlimit(
                Rlimit {
                    current: Some(300),
                    maximum: Some(300),
                },
                600,
            ),
            Rlimit {
                current: Some(300),
                maximum: Some(300),
            }
        );
        assert_eq!(
            clamp_rlimit(
                Rlimit {
                    current: Some(10),
                    maximum: None,
                },
                600,
            ),
            Rlimit {
                current: Some(10),
                maximum: Some(600),
            }
        );
        assert_eq!(
            clamp_rlimit(
                Rlimit {
                    current: Some(1_000),
                    maximum: Some(2_000),
                },
                600,
            ),
            Rlimit {
                current: Some(600),
                maximum: Some(600),
            }
        );
        assert_eq!(
            clamp_rlimit(
                Rlimit {
                    current: Some(10),
                    maximum: Some(100),
                },
                0,
            ),
            Rlimit {
                current: Some(0),
                maximum: Some(0),
            }
        );
    }
}
