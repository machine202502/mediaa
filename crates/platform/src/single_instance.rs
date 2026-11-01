pub struct SingleInstance {
    _guard: imp::Guard,
}

impl SingleInstance {
    pub fn acquire() -> anyhow::Result<Option<Self>> {
        Self::acquire_scoped(mediaa_core::dev::instance_scope())
    }

    fn acquire_scoped(scope: Option<String>) -> anyhow::Result<Option<Self>> {
        Ok(imp::Guard::acquire(scope)?.map(|guard| Self { _guard: guard }))
    }

    pub fn activate_existing() {
        imp::activate_existing();
    }

    pub fn take_activation(&self) -> bool {
        self._guard.take_activation()
    }
}

#[cfg(windows)]
use crate::windows::single_instance as imp;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::unix::single_instance as imp;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    pub struct Guard;

    impl Guard {
        pub fn acquire(_scope: Option<String>) -> anyhow::Result<Option<Self>> {
            panic!("MediaA supports Windows only")
        }

        pub fn take_activation(&self) -> bool {
            false
        }
    }

    pub fn activate_existing() {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_second_instance_guard_is_rejected_until_the_first_is_dropped() {
        let scope = Some("unit-test-single-instance".to_string());
        let first = super::SingleInstance::acquire_scoped(scope.clone()).unwrap();
        let Some(first) = first else {
            return;
        };
        assert!(super::SingleInstance::acquire_scoped(scope.clone())
            .unwrap()
            .is_none());
        #[cfg(windows)]
        {
            assert!(first.take_activation());
            assert!(!first.take_activation());
        }
        drop(first);
        assert!(super::SingleInstance::acquire_scoped(scope)
            .unwrap()
            .is_some());
    }

    #[test]
    fn development_scopes_are_isolated_and_have_separate_activation() {
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let first_scope = Some(format!("unit-test-first-{suffix}"));
        let second_scope = Some(format!("unit-test-second-{suffix}"));
        let first = super::SingleInstance::acquire_scoped(first_scope.clone()).unwrap();
        let second = super::SingleInstance::acquire_scoped(second_scope.clone()).unwrap();
        let (Some(first), Some(second)) = (first, second) else {
            return;
        };
        assert!(super::SingleInstance::acquire_scoped(first_scope.clone())
            .unwrap()
            .is_none());
        #[cfg(windows)]
        {
            assert!(first.take_activation());
            assert!(!first.take_activation());
            assert!(!second.take_activation());
        }
        assert!(super::SingleInstance::acquire_scoped(second_scope.clone())
            .unwrap()
            .is_none());
        #[cfg(windows)]
        {
            assert!(second.take_activation());
            assert!(!second.take_activation());
            assert!(!first.take_activation());
        }
        drop(first);
        assert!(super::SingleInstance::acquire_scoped(first_scope)
            .unwrap()
            .is_some());
        assert!(super::SingleInstance::acquire_scoped(second_scope)
            .unwrap()
            .is_none());
    }
}
