//! 后台任务给人看的样子：现在在做哪一步、做完了哪几步、刚下载了哪些文件、从哪下、多快、还要多久。
//!
//! 后台线程只管往外报 `Progress`；这里把一串进度攒成玩家关心的东西。下载页和更新弹窗都用它。

use crate::prelude::*;
use std::collections::VecDeque;

/// 最近下载的文件留几个。
const RECENT: usize = 7;
/// 用最近几秒的数据算速度：太短会跳，太长反应慢。
const SPEED_WINDOW: Duration = Duration::from_secs(3);

pub(crate) struct Step {
    pub(crate) name: String,
    started: Instant,
    /// 做完了用了多久；正在做的为 None。
    pub(crate) took: Option<Duration>,
}

pub(crate) struct Activity {
    /// 例如「1.21.1 · NeoForge 21.1.264」。
    pub(crate) title: String,
    pub(crate) progress: Progress,
    pub(crate) steps: Vec<Step>,
    /// 最近下载的文件，最新的在前。
    pub(crate) recent: VecDeque<String>,
    /// 最近一次下载用的主机。
    pub(crate) source: Option<String>,
    samples: VecDeque<(Instant, u64)>,
}

impl Activity {
    pub(crate) fn new(title: impl Into<String>, stage: impl Into<String>) -> Self {
        let mut activity = Self {
            title: title.into(),
            progress: Progress::default(),
            steps: Vec::new(),
            recent: VecDeque::new(),
            source: None,
            samples: VecDeque::new(),
        };
        activity.update(Progress::stage(stage));
        activity
    }

    pub(crate) fn update(&mut self, p: Progress) {
        let now = Instant::now();
        if self.steps.last().is_none_or(|step| step.name != p.stage) {
            if let Some(last) = self.steps.last_mut()
                && last.took.is_none()
            {
                last.took = Some(now - last.started);
            }
            self.steps.push(Step {
                name: p.stage.clone(),
                started: now,
                took: None,
            });
            // 每一步的字节数从 0 重新数，速度也从头算。
            self.samples.clear();
        }
        if let Some(current) = &p.current
            && self.recent.front() != Some(current)
        {
            self.recent.retain(|name| name != current);
            self.recent.push_front(current.clone());
            self.recent.truncate(RECENT);
        }
        if p.source.is_some() {
            self.source = p.source.clone();
        }
        if p.bytes_total > 0 || p.bytes_done > 0 {
            self.samples.push_back((now, p.bytes_done));
            while self
                .samples
                .front()
                .is_some_and(|(at, _)| now - *at > SPEED_WINDOW)
                && self.samples.len() > 2
            {
                self.samples.pop_front();
            }
        }
        self.progress = p;
    }

    /// 每秒字节数；数据不够时为 None。
    pub(crate) fn speed(&self) -> Option<f64> {
        let (first, last) = (self.samples.front()?, self.samples.back()?);
        let secs = (last.0 - first.0).as_secs_f64();
        (secs >= 0.5).then(|| last.1.saturating_sub(first.1) as f64 / secs)
    }

    /// 这一步还要多少秒；只在知道总大小、而且在动的时候算。
    pub(crate) fn eta(&self) -> Option<f64> {
        let p = &self.progress;
        let speed = self.speed().filter(|s| *s > 1.0)?;
        (p.bytes_total > p.bytes_done).then(|| (p.bytes_total - p.bytes_done) as f64 / speed)
    }

    /// 下载源的名字：玩家认得的是「Mojang 官方」「BMCLAPI 镜像」，不是主机名。
    pub(crate) fn source_label(&self) -> Option<String> {
        self.source.as_deref().map(source_name)
    }
}

pub(crate) fn source_name(host: &str) -> String {
    let named = [
        ("bmclapi", "BMCLAPI 镜像"),
        ("mojang.com", "Mojang 官方"),
        ("minecraft.net", "Mojang 官方"),
        ("neoforged.net", "NeoForge 官方"),
        ("fabricmc.net", "Fabric 官方"),
        ("minecraftforge.net", "Forge 官方"),
        ("modrinth.com", "Modrinth"),
        ("forgecdn.net", "CurseForge"),
        ("curseforge.com", "CurseForge"),
        ("github", "GitHub"),
    ];
    named
        .iter()
        .find(|(key, _)| host.contains(key))
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| host.to_string())
}

/// 「12.4 MB/s」。
pub(crate) fn rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bytes_per_sec / 1024.0 / 1024.0)
    } else {
        format!("{:.0} KB/s", bytes_per_sec / 1024.0)
    }
}

/// 「还要约 11 秒」「还要约 3 分钟」。
pub(crate) fn remaining(secs: f64) -> String {
    match secs {
        s if s < 1.5 => "马上就好".into(),
        s if s < 90.0 => format!("还要约 {:.0} 秒", s.ceil()),
        s => format!("还要约 {:.0} 分钟", (s / 60.0).ceil()),
    }
}

/// 「0.4 秒」「1 分 12 秒」。
pub(crate) fn took(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.1} 秒")
    } else if secs < 60.0 {
        format!("{secs:.0} 秒")
    } else {
        format!("{} 分 {} 秒", d.as_secs() / 60, d.as_secs() % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(stage: &str, done: u64, total: u64, current: Option<&str>) -> Progress {
        Progress {
            stage: stage.into(),
            bytes_done: done,
            bytes_total: total,
            current: current.map(String::from),
            source: current.map(|_| "piston-data.mojang.com".into()),
            ..Default::default()
        }
    }

    #[test]
    fn records_steps_files_and_source() {
        let mut a = Activity::new("1.21.1", "解析版本");
        a.update(p("下载依赖库", 0, 100, Some("lwjgl.jar")));
        a.update(p("下载依赖库", 40, 100, Some("gson.jar")));
        a.update(p("下载依赖库", 50, 100, Some("lwjgl.jar")));
        assert_eq!(a.steps.len(), 2);
        assert!(a.steps[0].took.is_some());
        assert!(a.steps[1].took.is_none());
        // 同一个文件再报一次就挪到最前面，不重复。
        assert_eq!(a.recent, ["lwjgl.jar", "gson.jar"]);
        assert_eq!(a.source_label().as_deref(), Some("Mojang 官方"));
        a.update(p("下载游戏资源", 0, 10, None));
        assert_eq!(a.steps.len(), 3);
        assert_eq!(a.speed(), None);
    }

    #[test]
    fn names_sources_and_formats_numbers() {
        assert_eq!(source_name("bmclapi2.bangbang93.com"), "BMCLAPI 镜像");
        assert_eq!(source_name("cdn.modrinth.com"), "Modrinth");
        assert_eq!(source_name("example.org"), "example.org");
        assert_eq!(rate(13.0 * 1024.0 * 1024.0), "13.0 MB/s");
        assert_eq!(rate(512.0 * 1024.0), "512 KB/s");
        assert_eq!(remaining(10.2), "还要约 11 秒");
        assert_eq!(remaining(200.0), "还要约 4 分钟");
        assert_eq!(took(Duration::from_millis(400)), "0.4 秒");
        assert_eq!(took(Duration::from_secs(72)), "1 分 12 秒");
    }
}
