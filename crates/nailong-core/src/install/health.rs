use crate::Root;
use crate::manifest::{self, Context, library_specs_for};

/// Cheap completeness check: existence and, where known, file size.
/// It deliberately does not hash multi-hundred-MB assets on every startup.
#[derive(Debug, Clone, Default)]
pub struct Health {
    pub total: u64,
    pub missing: u64,
    pub bytes_missing: u64,
}

impl Health {
    pub fn is_healthy(&self) -> bool {
        self.missing == 0
    }
}

pub fn inspect(root: &Root, id: &str) -> Health {
    let mut health = Health::default();
    health.total += 1;
    if !root.version_json(id).exists() {
        health.missing += 1;
        return health;
    }
    let (merged, chain) = match manifest::merged_chain(root, id) {
        Ok(v) => v,
        Err(_) => {
            health.missing += 1;
            return health;
        }
    };
    let ctx = Context::current();

    let client_id = merged
        .jar
        .clone()
        .or_else(|| {
            chain.iter().rev().find_map(|v| {
                let m = manifest::load(root, v).ok()?;
                if m.downloads.client.is_some() {
                    Some(v.clone())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| chain.first().cloned().unwrap_or_else(|| id.to_string()));
    health.total += 1;
    if !root.client_jar(&client_id).is_file() {
        health.missing += 1;
    }

    for spec in library_specs_for(root, &merged.libraries, &ctx) {
        health.total += 1;
        let known_size = spec.size;
        let ok = spec.dest.is_file()
            && known_size
                .is_none_or(|s| spec.dest.metadata().map(|m| m.len() == s).unwrap_or(false));
        if !ok {
            health.missing += 1;
            health.bytes_missing += known_size.unwrap_or(0);
        }
    }

    if let Some(ai) = &merged.asset_index {
        let index_path = root.indexes().join(format!("{}.json", ai.id));
        health.total += 1;
        if !index_path.is_file() {
            health.missing += 1;
            health.bytes_missing += ai.total_size.unwrap_or(0);
        } else if let Ok(data) = std::fs::read(&index_path)
            && let Ok(index) = serde_json::from_slice::<serde_json::Value>(&data)
            && let Some(objects) = index.get("objects").and_then(|v| v.as_object())
        {
            for obj in objects.values() {
                let Some(hash) = obj.get("hash").and_then(|v| v.as_str()) else {
                    continue;
                };
                let size = obj.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                let dest = root.objects().join(&hash[0..2]).join(hash);
                health.total += 1;
                let ok =
                    dest.is_file() && dest.metadata().map(|m| m.len() == size).unwrap_or(false);
                if !ok {
                    health.missing += 1;
                    health.bytes_missing += size;
                }
            }
        }
    }

    if let Some(logging) = &merged.logging
        && let Some(client) = &logging.client
    {
        let path = root.assets().join("log_configs").join(&client.file.id);
        health.total += 1;
        if !path.is_file() {
            health.missing += 1;
            health.bytes_missing += client.file.size.unwrap_or(0);
        }
    }

    health
}
