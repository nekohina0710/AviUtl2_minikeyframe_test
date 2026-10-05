use aviutl2::AnyResult;
use std::path::PathBuf;
use std::sync::OnceLock;

mod gui;

pub(crate) static EDIT_HANDLE: aviutl2::generic::GlobalEditHandle =
    aviutl2::generic::GlobalEditHandle::new();

/// 再生側スクリプトが読む keyframes.lua の場所（register時に決まる）。
pub(crate) static LUA_PATH: OnceLock<PathBuf> = OnceLock::new();

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
        let lua = aviutl2::config::app_data_path()
            .join("Script")
            .join("MiniKeyframe")
            .join("keyframes.lua");
        tracing::info!("keyframes.lua の出力先: {lua:?}");
        let _ = LUA_PATH.set(lua);
        match self.window.handle() {
            Ok(handle) => match registry.register_window_client("MiniKeyframe タイムライン", &handle)
            {
                Ok(()) => tracing::info!("パネルを登録しました: タイムライン"),
                Err(e) => tracing::error!("パネルの登録に失敗しました: {e:?}"),
            },
            Err(e) => tracing::error!("パネルの初期化に失敗しました: {e:?}"),
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

aviutl2::register_generic_plugin!(MiniKeyframeTimelinePlugin);
