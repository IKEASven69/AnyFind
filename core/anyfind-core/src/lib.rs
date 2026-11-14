//! anyfind-core：统一搜索内核（文件名 / 内容 / 六家 harness 会话 / skills / 记忆）。
//! 逻辑移植自 D:/coding/filefind 的 Python 参考实现（session_index.py /
//! agent_files_index.py / server.py / mft_index.py）。

pub mod adapters;
pub mod agent_files;
pub mod db;
pub mod dsl;
pub mod file_ops;
pub mod message;
pub mod paths;
pub mod resume;
pub mod scanner;
pub mod search;
pub mod util;

#[cfg(windows)]
pub mod mft;
#[cfg(windows)]
pub mod usn;
pub mod name_index;
