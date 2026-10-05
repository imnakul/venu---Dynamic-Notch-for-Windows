//! Pure launch-mode selection so startup behavior can be checked without
//! starting a Windows window or tray thread.

pub fn settings_visible_from_args<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|arg| arg.as_ref() == "--settings")
}

#[cfg(test)]
mod tests {
    use super::settings_visible_from_args;

    #[test]
    fn ordinary_launch_starts_quiet_and_settings_flag_opens_window() {
        assert!(!settings_visible_from_args(std::iter::empty::<&str>()));
        assert!(settings_visible_from_args(["--settings"]));
    }

    #[test]
    fn startup_and_minimized_keep_the_quiet_behavior() {
        assert!(!settings_visible_from_args(["--startup"]));
        assert!(!settings_visible_from_args(["--minimized"]));
    }

    #[test]
    fn explicit_settings_flag_wins_when_quiet_flags_are_also_present() {
        assert!(settings_visible_from_args(["--startup", "--settings"]));
        assert!(settings_visible_from_args(["--minimized", "--settings"]));
    }

    #[test]
    fn unrelated_arguments_do_not_open_settings() {
        assert!(!settings_visible_from_args(["--something-else"]));
    }
}
