use crate::roles::Role;
use mediaa_core::paths;
use mediaa_frontend::session;

fn log_file(role: Role) -> &'static str {
    match role {
        Role::Daemon => "mediaa.log",
        Role::Ui => "mediaa-ui.log",
        Role::Launcher => "mediaa-launcher.log",
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let role = Role::from_args(&args);
    mediaa_core::logging::init_named(&paths::logs_dir(), log_file(role), log::LevelFilter::Info);
    let forwarded = role.strip(&args);
    log::info!(
        "{} {} starting as {role:?}",
        paths::APP_IDENTITY.name,
        env!("CARGO_PKG_VERSION")
    );
    let result = match role {
        Role::Daemon => mediaa_backend::daemon::run(),
        Role::Ui => session::run_ui(&forwarded),
        Role::Launcher => mediaa_backend::daemon::launch(&forwarded),
    };
    if let Err(error) = result {
        log::error!("{error:#}");
        if role != Role::Daemon {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::log_file;
    use crate::roles::Role;

    #[test]
    fn each_role_writes_its_own_log_file() {
        assert_eq!(log_file(Role::Daemon), "mediaa.log");
        assert_eq!(log_file(Role::Ui), "mediaa-ui.log");
        assert_eq!(log_file(Role::Launcher), "mediaa-launcher.log");
    }
}
