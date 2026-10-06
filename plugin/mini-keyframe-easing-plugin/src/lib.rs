use aviutl2::AnyResult;
use std::path::PathBuf;
use std::sync::OnceLock;

mod gui;

/// 再生側スクリプトが読む keyframes.lua の場所（register時に決まる）。
pub(crate) static LUA_PATH: OnceLock<PathBuf> = OnceLock::new();

#[aviutl2::plugin(GenericPlugin)]
pub struct MiniKeyframeEasingPlugin {
    window: aviutl2_eframe::EframeWindow,
}
unsafe impl Send for MiniKeyframeEasingPlugin {}
unsafe impl Sync for MiniKeyframeEasingPlugin {}

impl aviutl2::generic::GenericPlugin for MiniKeyframeEasingPlugin {
    fn new(_info: aviutl2::AviUtl2Info) -> AnyResult<Self> {
        Self::init_logging();
        tracing::info!("Initializing MiniKeyframe Easing...");
        let window =
            aviutl2_eframe::EframeWindow::new("MiniKeyframeEasing", move |cc, _handle| {
                Ok(Box::new(gui::EasingApp::new(cc)))
            })?;
        Ok(Self { window })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "MiniKeyframe Easing".to_string(),
            information: format!("Easing panel / v{}", env!("CARGO_PKG_VERSION")),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        let lua = aviutl2::config::app_data_path()
            .join("Script")
            .join("MiniKeyframe")
            .join("keyframes.lua");
        let _ = LUA_PATH.set(lua);
        match self.window.handle() {
            Ok(handle) => match registry.register_window_client("MiniKeyframe イージング", &handle)
            {
                Ok(()) => tracing::info!("パネルを登録しました: イージング"),
                Err(e) => tracing::error!("パネルの登録に失敗しました: {e:?}"),
            },
            Err(e) => tracing::error!("パネルの初期化に失敗しました: {e:?}"),
        }
    }
}

impl MiniKeyframeEasingPlugin {
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

aviutl2::register_generic_plugin!(MiniKeyframeEasingPlugin);
