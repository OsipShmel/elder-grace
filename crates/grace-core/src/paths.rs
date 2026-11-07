//! Общие пути рантайма.

use std::path::PathBuf;

/// Unix-сокет управления рантаймом. Лежит во временном каталоге пользователя:
/// он одинаков для `.app`, запущенного из Finder, и для `grace-ctl` в терминале,
/// а путь не зависит от того, где лежит репозиторий.
pub fn control_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("GRACE_SOCKET") {
        return PathBuf::from(p);
    }
    std::env::temp_dir().join("elder-grace").join("app.sock")
}
