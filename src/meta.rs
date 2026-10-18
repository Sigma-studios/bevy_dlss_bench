//! `out/<scene>/<variant>.meta`: what a capture process actually did, as `key=value` lines.
//! Written by the capture process, read back by the orchestrator to build the sheets.

use std::{fs, path::Path};

use bevy::math::UVec2;

#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// `ok`, `dlss-unsupported: <reason>`, `dlss-inactive ...` or `failed: <reason>`.
    pub status: String,
    pub scene: String,
    pub title: String,
    pub feature: String,
    pub variant: String,
    pub variant_desc: String,
    pub expected: String,
    pub pipeline: String,
    pub output: UVec2,
    pub render: Option<UVec2>,
    pub adapter: String,
    pub features: Vec<String>,
    pub frames: u32,
}

fn res(r: UVec2) -> String {
    format!("{}x{}", r.x, r.y)
}

pub fn parse_res(s: &str) -> Option<UVec2> {
    let (w, h) = s.split_once('x')?;
    Some(UVec2::new(w.trim().parse().ok()?, h.trim().parse().ok()?))
}

impl Meta {
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }

    pub fn write(&self, path: &Path) {
        let one_line = |s: &str| s.replace('\n', " ");
        let text = format!(
            "status={}\nscene={}\ntitle={}\nfeature={}\nvariant={}\nvariant_desc={}\nexpected={}\n\
             pipeline={}\noutput={}\nrender={}\nadapter={}\nfeatures={}\nframes={}\n",
            one_line(&self.status),
            self.scene,
            self.title,
            one_line(&self.feature),
            self.variant,
            self.variant_desc,
            one_line(&self.expected),
            self.pipeline,
            res(self.output),
            self.render.map(res).unwrap_or_default(),
            one_line(&self.adapter),
            self.features.join("; "),
            self.frames,
        );
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Err(e) = fs::write(path, text) {
            eprintln!("cannot write {}: {e}", path.display());
        }
    }

    pub fn read(path: &Path) -> Option<Meta> {
        let text = fs::read_to_string(path).ok()?;
        let mut m = Meta::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.to_owned();
            match k {
                "status" => m.status = v,
                "scene" => m.scene = v,
                "title" => m.title = v,
                "feature" => m.feature = v,
                "variant" => m.variant = v,
                "variant_desc" => m.variant_desc = v,
                "expected" => m.expected = v,
                "pipeline" => m.pipeline = v,
                "output" => m.output = parse_res(&v).unwrap_or_default(),
                "render" => m.render = parse_res(&v),
                "adapter" => m.adapter = v,
                "features" => {
                    m.features = v
                        .split("; ")
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                }
                "frames" => m.frames = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        Some(m)
    }
}
