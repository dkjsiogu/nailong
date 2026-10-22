use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("读写失败 {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("解析 JSON 失败 {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("HTTP {status}: {url}")]
    Http { url: String, status: u16 },

    #[error("网络错误 {url}: {source}")]
    Net {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("校验失败 {path}: 期望 {expected}, 实际 {actual}")]
    Checksum {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("缺少文件: {0}")]
    MissingFile(PathBuf),

    #[error("找不到版本: {0}")]
    VersionNotFound(String),

    #[error("版本继承成环: {0}")]
    CircularInheritance(String),

    #[error("版本缺少 mainClass: {0}")]
    NoMainClass(String),

    #[error("不支持: {0}")]
    Unsupported(String),

    #[error("安装失败: {0}")]
    Install(String),

    #[error("找不到 Java {0}")]
    JavaNotFound(u32),

    #[error("启动失败: {0}")]
    Launch(String),

    #[error("操作已取消")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source,
    }
}

pub fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Error {
    Error::Json {
        path: path.into(),
        source,
    }
}
