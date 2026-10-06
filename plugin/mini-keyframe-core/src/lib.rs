//! タイムラインとイージングの2つのプラグインで共有するデータと処理。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// 区間（その点から次の点まで）の動き方。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Ease {
    Linear,
    Hold,
    /// CSSのcubic-bezier(x1, y1, x2, y2)と同じ。
    Bezier([f64; 4]),
}

impl Ease {
    /// 時間の割合r(0..1)から、進み具合(0..1)を返す。
    pub fn at(&self, r: f64) -> f64 {
        match self {
            Ease::Linear => r,
            Ease::Hold => 0.0,
            Ease::Bezier(p) => bezier(r, p[0], p[1], p[2], p[3]),
        }
    }
}

pub fn bezier(x: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let f = |u: f64, a: f64, b: f64| {
        let v = 1.0 - u;
        3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
    };
    let (mut lo, mut hi, mut u) = (0.0, 1.0, x);
    for _ in 0..30 {
        u = (lo + hi) / 2.0;
        if f(u, x1, x2) < x {
            lo = u;
        } else {
            hi = u;
        }
    }
    f(u, y1, y2)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Key {
    /// 時間（秒、オブジェクトの先頭から）
    pub t: f64,
    pub v: f64,
    /// この点から次の点までの動き方
    pub ease: Ease,
}

/// キーフレームの値を、時間tで評価する（keyframes.lua側と同じ式）。
pub fn eval_keys(keys: &[Key], t: f64) -> Option<f64> {
    let first = keys.first()?;
    let last = keys.last()?;
    if t <= first.t {
        return Some(first.v);
    }
    for w in keys.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if t < b.t {
            let span = b.t - a.t;
            if span <= 0.0 {
                return Some(b.v);
            }
            return Some(a.v + (b.v - a.v) * a.ease.at((t - a.t) / span));
        }
    }
    Some(last.v)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Doc {
    /// IDごとのキーフレーム列（時間の昇順）
    pub tracks: BTreeMap<u32, Vec<Key>>,
}

impl Doc {
    pub fn next_id(&self) -> u32 {
        let mut n = 1;
        while self.tracks.contains_key(&n) {
            n += 1;
        }
        n
    }
}

/// 選択中のID・キーフレーム（キーのindexは「その点から始まる区間」も表す）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Selection {
    pub id: Option<u32>,
    pub key: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub doc: Doc,
    pub sel: Selection,
    /// 元に戻す用の履歴（古い順）
    #[serde(default)]
    pub undo: Vec<Doc>,
    /// やり直し用の履歴
    #[serde(default)]
    pub redo: Vec<Doc>,
}

const HISTORY_MAX: usize = 100;

fn sample_state() -> State {
    let mut doc = Doc::default();
    doc.tracks.insert(
        1,
        vec![
            Key { t: 0.0, v: 0.0, ease: Ease::Linear },
            Key { t: 1.0, v: 100.0, ease: Ease::Linear },
        ],
    );
    State {
        doc,
        sel: Selection { id: Some(1), key: None },
        undo: Vec::new(),
        redo: Vec::new(),
    }
}

/// 新しいプロジェクト用の初期データ（ID 1 に2点だけ）。
pub fn default_doc() -> Doc {
    sample_state().doc
}

/// 2つのプラグインが共有する状態ファイル。
pub fn state_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("MiniKeyframe").join("state.json")
}

pub fn to_lua(doc: &Doc) -> String {
    fn n(x: f64) -> String {
        format!("{}", (x * 1e4).round() / 1e4)
    }
    let mut s = String::from(
        "-- MiniKeyframe のデータ（MiniKeyframe プラグインが自動で書き出します）\nreturn {\n",
    );
    for (id, keys) in &doc.tracks {
        s.push_str(&format!("  [{id}] = {{\n"));
        for k in keys {
            let e = match &k.ease {
                Ease::Linear => String::new(),
                Ease::Hold => ", ease = \"hold\"".to_string(),
                Ease::Bezier(p) => {
                    format!(", ease = {{{}, {}, {}, {}}}", n(p[0]), n(p[1]), n(p[2]), n(p[3]))
                }
            };
            s.push_str(&format!("    {{ t = {}, v = {}{} }},\n", n(k.t), n(k.v), e));
        }
        s.push_str("  },\n");
    }
    s.push_str("}\n");
    s
}

/// 状態ファイルの読み書きと、他プラグインによる変更の検知。
pub struct Store {
    pub state: State,
    path: PathBuf,
    mtime: Option<SystemTime>,
    last_tag: String,
    last_at: Option<Instant>,
}

impl Store {
    pub fn open() -> Self {
        let mut s = Store {
            state: State::default(),
            path: state_path(),
            mtime: None,
            last_tag: String::new(),
            last_at: None,
        };
        if !s.reload() {
            s.state = sample_state();
        }
        s
    }

    fn file_mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path).ok()?.modified().ok()
    }

    fn reload(&mut self) -> bool {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return false;
        };
        let Ok(state) = serde_json::from_str::<State>(&text) else {
            return false;
        };
        self.state = state;
        self.mtime = self.file_mtime();
        true
    }

    /// 編集の履歴を記録する。`before`は編集する前のデータ。
    ///
    /// `tag`が空でなく、直前の記録と同じtagで短い間隔のときは、1回の操作とみなして
    /// 新しい履歴を作らない（数値のドラッグなど、連続する変更をまとめるため）。
    pub fn record(&mut self, before: Doc, tag: &str) {
        let now = Instant::now();
        let merge = !tag.is_empty()
            && self.last_tag == tag
            && self
                .last_at
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(800));
        self.last_tag = tag.to_string();
        self.last_at = Some(now);
        if merge {
            return;
        }
        self.state.undo.push(before);
        if self.state.undo.len() > HISTORY_MAX {
            self.state.undo.remove(0);
        }
        self.state.redo.clear();
    }

    /// 元に戻す。戻せたらtrue。
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.state.undo.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.state.doc, prev);
        self.state.redo.push(cur);
        self.last_tag.clear();
        self.fix_selection();
        true
    }

    /// やり直す。やり直せたらtrue。
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.state.redo.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.state.doc, next);
        self.state.undo.push(cur);
        self.last_tag.clear();
        self.fix_selection();
        true
    }

    /// 履歴を空にする（プロジェクトを開いたときなど）。
    pub fn clear_history(&mut self) {
        self.state.undo.clear();
        self.state.redo.clear();
        self.last_tag.clear();
    }

    fn fix_selection(&mut self) {
        let sel = &mut self.state.sel;
        if let Some(id) = sel.id {
            match self.state.doc.tracks.get(&id) {
                None => *sel = Selection::default(),
                Some(keys) => {
                    if sel.key.is_some_and(|j| j >= keys.len()) {
                        sel.key = None;
                    }
                }
            }
        }
    }

    /// 他のプラグインがファイルを書き換えていたら読み直す。変わったらtrue。
    pub fn poll(&mut self) -> bool {
        let m = self.file_mtime();
        if m.is_some() && m != self.mtime {
            self.reload()
        } else {
            false
        }
    }

    /// 状態を保存する。`lua`を渡すと、再生側スクリプト用のkeyframes.luaも書き出す。
    pub fn save(&mut self, lua: Option<&Path>) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string(&self.state).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        self.mtime = self.file_mtime();
        if let Some(p) = lua {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(p, to_lua(&self.state.doc))
                .map_err(|e| format!("keyframes.lua の書き込みに失敗: {e}"))?;
        }
        Ok(())
    }
}
