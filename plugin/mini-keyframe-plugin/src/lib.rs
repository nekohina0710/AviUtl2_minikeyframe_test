use aviutl2::AnyResult;
use mini_keyframe_core::{Doc, Selection, Store};
use std::path::PathBuf;
use std::sync::OnceLock;

mod gui;

pub(crate) static EDIT_HANDLE: aviutl2::generic::GlobalEditHandle =
    aviutl2::generic::GlobalEditHandle::new();

/// 再生側スクリプトが読む keyframes.lua の場所（register時に決まる）。
pub(crate) static LUA_PATH: OnceLock<PathBuf> = OnceLock::new();

/// プロジェクトファイルに保存するときのキー。
const PROJECT_KEY: &str = "mini_keyframe_doc";

/// keyframes.lua の場所。AviUtl2のデータフォルダ内（設定の準備ができてから呼ぶこと）。
pub(crate) fn lua_path() -> &'static PathBuf {
    LUA_PATH.get_or_init(|| {
        aviutl2::config::app_data_path()
            .join("Script")
            .join("MiniKeyframe")
            .join("keyframes.lua")
    })
}

#[aviutl2::plugin(GenericPlugin)]
pub struct MiniKeyframeTimelinePlugin {
    window: aviutl2_eframe::EframeWindow,
}
unsafe impl Send for MiniKeyframeTimelinePlugin {}
unsafe impl Sync for MiniKeyframeTimelinePlugin {}

impl aviutl2::generic::GenericPlugin for MiniKeyframeTimelinePlugin {
    fn new(_info: aviutl2::AviUtl2Info) -> AnyResult<Self> {
        Self::init_logging();
        tracing::info!("Initializing MiniKeyframe Timeline...");
        let window =
            aviutl2_eframe::EframeWindow::new("MiniKeyframeTimeline", move |cc, _handle| {
                Ok(Box::new(gui::TimelineApp::new(cc)))
            })?;
        Ok(Self { window })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "MiniKeyframe Timeline".to_string(),
            information: format!("Keyframe timeline panel / v{}", env!("CARGO_PKG_VERSION")),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        EDIT_HANDLE.init(registry.create_edit_handle());
        tracing::info!("keyframes.lua の出力先: {:?}", lua_path());
        match self.window.handle() {
            Ok(handle) => match registry.register_window_client("MiniKeyframe タイムライン", &handle)
            {
                Ok(()) => tracing::info!("パネルを登録しました: タイムライン"),
                Err(e) => tracing::error!("パネルの登録に失敗しました: {e:?}"),
            },
            Err(e) => tracing::error!("パネルの初期化に失敗しました: {e:?}"),
        }
    }

    /// プロジェクトを開いたとき（新規作成時も呼ばれる）：保存されたキーフレームを復元する。
    fn on_project_load(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        let doc = match project.deserialize::<Doc>(PROJECT_KEY) {
            Ok(doc) => {
                tracing::info!("プロジェクトからキーフレームを読み込みました（{} 個のID）", doc.tracks.len());
                doc
            }
            Err(e) => {
                tracing::info!("プロジェクトにキーフレームのデータがないため、初期状態にします: {e}");
                mini_keyframe_core::default_doc()
            }
        };
        // 共有ファイルを書き換える。各パネルは、次の確認のときに読み直す。
        let mut store = Store::open();
        store.state.doc = doc;
        store.state.sel = Selection::default();
        store.clear_history();
        if let Err(e) = store.save(Some(lua_path().as_path())) {
            tracing::error!("キーフレームの反映に失敗しました: {e}");
        }
    }

    /// プロジェクトを保存する直前：共有ファイルの最新の内容を、プロジェクトに入れる。
    fn on_project_save(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        let store = Store::open();
        if let Err(e) = project.serialize(PROJECT_KEY, &store.state.doc) {
            tracing::error!("プロジェクトへのキーフレームの保存に失敗しました: {e}");
        }
    }
}

impl MiniKeyframeTimelinePlugin {
    fn init_logging() {
        aviutl2::tracing_subscriber::fmt()
            .with_max_level(if cfg!(debug_assertions) {
                tracing::Level::DEBUG
            } else {
                tracing::Level::INFO
            })
            .event_format(aviutl2::logger::AviUtl2Formatter)
            .with_writer(aviutl2::logger::AviUtl2LogWriter)
            .init();
}
}

/// 現在オブジェクト設定ウィンドウで開いているオブジェクトの長さ（秒）。
pub(crate) fn object_length_sec() -> Result<f64, String> {
    if !EDIT_HANDLE.is_ready() {
        return Err("編集ハンドルがまだ初期化されていません".to_string());
    }
    let fps = EDIT_HANDLE.get_edit_info().fps;
    let fps_f = *fps.numer() as f64 / *fps.denom() as f64;
    let result = EDIT_HANDLE.call_read_section(move |section| -> Result<f64, String> {
        let object = match section.get_focused_object() {
            Ok(Some(object)) => object,
            Ok(None) => return Err("オブジェクトが選択されていません".to_string()),
            Err(e) => return Err(format!("選択オブジェクトの取得に失敗: {e:?}")),
        };
        let lf = section
            .get_object_layer_frame(object)
            .map_err(|e| format!("フレーム情報の取得に失敗: {e:?}"))?;
        Ok((lf.end + 1 - lf.start) as f64 / fps_f)
    });
    match result {
        Ok(inner) => inner,
        Err(e) => Err(format!("読み取りセクションの呼び出しに失敗: {e:?}")),
    }
}

/// シーンのfps。
pub(crate) fn scene_fps() -> Option<f64> {
    if !EDIT_HANDLE.is_ready() {
        return None;
    }
    let fps = EDIT_HANDLE.get_edit_info().fps;
    Some(*fps.numer() as f64 / *fps.denom() as f64)
}

/// 現在のカーソル（再生位置）のフレームと、シーンのfps。
pub(crate) fn cursor_frame() -> Option<(usize, f64)> {
    if !EDIT_HANDLE.is_ready() {
        return None;
    }
    let info = EDIT_HANDLE.get_edit_info();
    let fps = *info.fps.numer() as f64 / *info.fps.denom() as f64;
    Some((info.frame, fps))
}

/// 選択中のオブジェクトの開始フレーム。
pub(crate) fn focused_object_start() -> Option<usize> {
    if !EDIT_HANDLE.is_ready() {
        return None;
    }
    EDIT_HANDLE
        .call_read_section(|section| -> Option<usize> {
            let object = section.get_focused_object().ok().flatten()?;
            section.get_object_layer_frame(object).ok().map(|lf| lf.start)
        })
        .ok()
        .flatten()
}

aviutl2::register_generic_plugin!(MiniKeyframeTimelinePlugin);
