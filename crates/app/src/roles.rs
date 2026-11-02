#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Launcher,
    Daemon,
    Ui,
}

impl Role {
    pub fn from_args(args: &[String]) -> Self {
        for arg in args {
            match arg.as_str() {
                "--daemon" | "--backend" => return Self::Daemon,
                "--ui" => return Self::Ui,
                _ => {}
            }
        }
        Self::Launcher
    }

    pub fn strip(self, args: &[String]) -> Vec<String> {
        args.iter()
            .filter(|arg| {
                arg.as_str() != "--daemon" && arg.as_str() != "--backend" && arg.as_str() != "--ui"
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Role;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn roles_are_read_from_the_command_line() {
        assert_eq!(Role::from_args(&args(&[])), Role::Launcher);
        assert_eq!(Role::from_args(&args(&["--page=library"])), Role::Launcher);
        assert_eq!(Role::from_args(&args(&["--daemon"])), Role::Daemon);
        assert_eq!(Role::from_args(&args(&["--backend"])), Role::Daemon);
        assert_eq!(
            Role::from_args(&args(&["--page=library", "--ui"])),
            Role::Ui
        );
    }

    #[test]
    fn the_role_flag_is_removed_before_the_arguments_are_forwarded() {
        let forwarded = Role::Ui.strip(&args(&["--ui", "--page=library", "--tray"]));
        assert_eq!(forwarded, args(&["--page=library", "--tray"]));
    }
}
