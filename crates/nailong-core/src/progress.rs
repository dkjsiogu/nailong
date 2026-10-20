use std::sync::Arc;

/// A progress update from install/download code.
///
/// `bytes_total` is zero when not every file in the batch has a known size;
/// in that case the UI should fall back to `files_done / files_total`.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub stage: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub indeterminate: bool,
}

impl Progress {
    pub fn stage(stage: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            indeterminate: true,
            ..Default::default()
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.bytes_total > 0 {
            (self.bytes_done as f32 / self.bytes_total as f32).clamp(0.0, 1.0)
        } else if self.files_total > 0 {
            (self.files_done as f32 / self.files_total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

pub type Sink = Arc<dyn Fn(Progress) + Send + Sync>;

pub fn noop() -> Sink {
    Arc::new(|_| {})
}

pub fn from_fn(f: impl Fn(Progress) + Send + Sync + 'static) -> Sink {
    Arc::new(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_prefers_bytes_then_files() {
        let mut p = Progress {
            bytes_done: 25,
            bytes_total: 100,
            files_done: 1,
            files_total: 10,
            ..Default::default()
        };
        assert!((p.fraction() - 0.25).abs() < f32::EPSILON);
        p.bytes_total = 0;
        assert!((p.fraction() - 0.1).abs() < f32::EPSILON);
    }
}
