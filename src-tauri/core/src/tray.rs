//! Top-bar status icon via the StatusNotifierItem D-Bus protocol (shown by Ubuntu's
//! AppIndicator extension). Pure D-Bus: no GTK is loaded into the daemon for it.

use std::sync::Arc;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{MenuItem, StandardItem};

#[derive(Debug, Clone, Copy)]
pub enum TrayAction {
    Open,
    TogglePause,
    Quit,
}

pub struct Tray {
    pub paused: bool,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
    icons: Vec<ksni::Icon>,
}

fn to_argb(png: &[u8]) -> Option<ksni::Icon> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for p in img.pixels() {
        data.extend_from_slice(&[p[3], p[0], p[1], p[2]]);
    }
    Some(ksni::Icon {
        width: w as i32,
        height: h as i32,
        data,
    })
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "clipvault".into()
    }
    fn title(&self) -> String {
        "ClipVault".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "ClipVault".into(),
            description: if self.paused {
                "Capture paused".into()
            } else {
                "Recording clipboard history".into()
            },
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on_action)(TrayAction::Open);
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, action: TrayAction| -> MenuItem<Self> {
            StandardItem {
                label: label.into(),
                activate: Box::new(move |t: &mut Self| (t.on_action)(action)),
                ..Default::default()
            }
            .into()
        };
        vec![
            item("Open ClipVault", TrayAction::Open),
            item(
                if self.paused {
                    "Resume capture"
                } else {
                    "Pause capture"
                },
                TrayAction::TogglePause,
            ),
            MenuItem::Separator,
            item("Quit ClipVault", TrayAction::Quit),
        ]
    }
}

pub fn spawn(
    paused: bool,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
) -> Option<Handle<Tray>> {
    let icons = [
        &include_bytes!("../../icons/32x32.png")[..],
        &include_bytes!("../../icons/64x64.png")[..],
    ]
    .iter()
    .filter_map(|b| to_argb(b))
    .collect();
    match (Tray {
        paused,
        on_action,
        icons,
    })
    .spawn()
    {
        Ok(h) => Some(h),
        Err(e) => {
            log::warn!("status icon unavailable: {e}");
            None
        }
    }
}
