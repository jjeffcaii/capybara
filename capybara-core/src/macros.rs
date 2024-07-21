#[macro_export]
macro_rules! xdebug (
    ($k:expr, #$tag:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            slog::log!(l, slog::Level::Debug, $tag, $($args)*)
        }
    };
    ($k:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            slog::log!(l, slog::Level::Debug, "", $($args)*)
        }
    };
);

#[macro_export]
macro_rules! xinfo (
    ($k:expr, #$tag:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            slog::log!(l, slog::Level::Info, $tag, $($args)*)
        }
    };
    ($k:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            slog::log!(l, slog::Level::Info, "", $($args)*)
        }
    };
);

#[macro_export]
macro_rules! xwarn (
    ($k:expr, #$tag:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            slog::log!(l, slog::Level::Warning, $tag, $($args)*)
        }
    };
    ($k:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            slog::log!(l, slog::Level::Warning, "", $($args)*)
        }
    };
);

#[macro_export]
macro_rules! xerror (
    ($k:expr, #$tag:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            logx::log!(l, slog::Level::Error, $tag, $($args)*)
        }
    };
    ($k:expr, $($args:tt)*) => {
        let r = $crate::logger::LOGGERS.read();
        if let Some(l) = r.get(&$k){
            logx::log!(l, slog::Level::Error, "", $($args)*)
        }
    };
);

#[cfg(test)]
mod logger_tests {
    use crate::logger::Key;

    #[test]
    fn test_logger() {
        xinfo!(Key::default(), "{}", "1234");
    }
}
