use super::Config;
use std::env;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock")
}

fn set_env(key: &str, value: Option<&str>) -> Option<String> {
    let previous = env::var(key).ok();
    match value {
        Some(value) => env::set_var(key, value),
        None => env::remove_var(key),
    }
    previous
}

fn restore_env(key: &str, previous: Option<String>) {
    match previous {
        Some(value) => env::set_var(key, value),
        None => env::remove_var(key),
    }
}

#[test]
fn default_config_dir_ignores_empty_xdg() {
    let _guard = env_lock();
    let home = env::var("HOME").unwrap_or_default();
    if home.trim().is_empty() {
        return;
    }
    let prev_xdg = set_env("XDG_CONFIG_HOME", Some(""));
    let prev_home = set_env("HOME", Some(&home));
    let dir = Config::default_config_dir().expect("config dir");
    assert_eq!(dir, PathBuf::from(home).join(".config").join("unixnotis"));
    restore_env("XDG_CONFIG_HOME", prev_xdg);
    restore_env("HOME", prev_home);
}

#[test]
fn default_config_dir_ignores_whitespace_xdg() {
    let _guard = env_lock();
    let home = env::var("HOME").unwrap_or_default();
    if home.trim().is_empty() {
        return;
    }
    let prev_xdg = set_env("XDG_CONFIG_HOME", Some("   "));
    let prev_home = set_env("HOME", Some(&home));
    let dir = Config::default_config_dir().expect("config dir");
    assert_eq!(dir, PathBuf::from(home).join(".config").join("unixnotis"));
    restore_env("XDG_CONFIG_HOME", prev_xdg);
    restore_env("HOME", prev_home);
}

#[test]
fn default_config_dir_ignores_relative_xdg() {
    let _guard = env_lock();
    let home = env::var("HOME").unwrap_or_default();
    if home.trim().is_empty() {
        return;
    }
    let prev_xdg = set_env("XDG_CONFIG_HOME", Some("relative/path"));
    let prev_home = set_env("HOME", Some(&home));
    let dir = Config::default_config_dir().expect("config dir");
    assert_eq!(dir, PathBuf::from(home).join(".config").join("unixnotis"));
    restore_env("XDG_CONFIG_HOME", prev_xdg);
    restore_env("HOME", prev_home);
}

#[test]
fn default_config_dir_accepts_absolute_xdg() {
    let _guard = env_lock();
    let home = env::var("HOME").unwrap_or_default();
    if home.trim().is_empty() {
        return;
    }
    let xdg = PathBuf::from(home.clone()).join(".config-test");
    let prev_xdg = set_env("XDG_CONFIG_HOME", Some(xdg.to_string_lossy().as_ref()));
    let prev_home = set_env("HOME", Some(&home));
    let dir = Config::default_config_dir().expect("config dir");
    assert_eq!(dir, xdg.join("unixnotis"));
    restore_env("XDG_CONFIG_HOME", prev_xdg);
    restore_env("HOME", prev_home);
}
