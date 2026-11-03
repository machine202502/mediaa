use std::path::PathBuf;

pub fn socket_name(scope: &str) -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\mediaa2-{scope}"))
}

pub fn prepare(_scope: &str) {}

#[cfg(test)]
mod tests {
    use super::socket_name;
    use std::path::PathBuf;

    #[test]
    fn the_endpoint_is_a_windows_named_pipe() {
        assert_eq!(
            socket_name("default"),
            PathBuf::from(r"\\.\pipe\mediaa2-default")
        );
        assert_eq!(
            socket_name("unit-test"),
            PathBuf::from(r"\\.\pipe\mediaa2-unit-test")
        );
    }
}
