//! Budget: how much memory local models may take (GPU, RAM) and how many jobs of each kind run
//! at once; the GPUs found on this computer.

use super::super::header;
use crate::app::App;
use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::WorkKind;
use rust_i18n::t;

/// A graphics card models can run on.
#[derive(Debug, Clone, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vendor: Vendor,
    pub discrete: bool,
    /// `(total, free)` bytes, when the model engine can use this card.
    pub memory: Option<(u64, u64)>,
}

/// What the machine offers models: the cards, and whether the engine can use any of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub gpus: Vec<Gpu>,
    /// The engine sees at least one GPU (built with a GPU backend and the driver works).
    pub engine_gpu: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    fn from_pci(id: u32) -> Vendor {
        match id {
            0x10de => Vendor::Nvidia,
            0x1002 | 0x1022 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            _ => Vendor::Other,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Vendor::Nvidia => "NVIDIA",
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Other => "",
        }
    }
}

/// Real GPUs from adapter descriptions `(name, PCI vendor, PCI device, kind)`: one per card
/// (Vulkan and DirectX list the same card twice), no software renderers.
pub fn real_gpus(adapters: impl IntoIterator<Item = (String, u32, u32, eframe::wgpu::DeviceType)>) -> Vec<Gpu> {
    use eframe::wgpu::DeviceType;
    let mut seen = Vec::new();
    let mut gpus = Vec::new();
    for (name, vendor, device, kind) in adapters {
        if !matches!(kind, DeviceType::DiscreteGpu | DeviceType::IntegratedGpu) || seen.contains(&(vendor, device)) {
            continue;
        }
        seen.push((vendor, device));
        gpus.push(Gpu { name, vendor: Vendor::from_pci(vendor), discrete: kind == DeviceType::DiscreteGpu, memory: None });
    }
    // Discrete cards first: that is where models go.
    gpus.sort_by_key(|g| !g.discrete);
    gpus
}

/// Gives each card the memory of the engine device with the same name.
pub fn attach_memory(gpus: &mut [Gpu], engine: &[mediaa_agent::llm::Device]) {
    for g in gpus {
        let name = g.name.to_lowercase();
        let same = |n: &String| {
            let n = n.to_lowercase();
            !n.is_empty() && (n.contains(&name) || name.contains(&n))
        };
        g.memory = engine.iter().find(|d| same(&d.description) || same(&d.name)).map(|d| (d.memory_total, d.memory_free));
    }
}

fn detect() -> Found {
    let instance = eframe::wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(eframe::wgpu::Backends::all()));
    let mut gpus = real_gpus(adapters.iter().map(|a| {
        let i = a.get_info();
        (i.name, i.vendor, i.device, i.device_type)
    }));
    let engine = mediaa_agent::llm::gpus();
    attach_memory(&mut gpus, &engine);
    Found { gpus, engine_gpu: !engine.is_empty() }
}

#[derive(Default)]
pub struct BudgetUi {
    found: Option<Found>,
    detecting: Option<Receiver<Found>>,
}

pub(super) fn kind_label(k: WorkKind) -> String {
    match k {
        WorkKind::Agent => t!("budget.kind_agent"),
        WorkKind::Transcription => t!("budget.kind_transcription"),
        WorkKind::Indexing => t!("budget.kind_indexing"),
        WorkKind::WebSearch => t!("budget.kind_web_search"),
        WorkKind::ProviderCalls => t!("budget.kind_provider_calls"),
    }
    .into()
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / f64::from(1u32 << 30))
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("budget.title"), "");
    let state = &mut app.agent.budget;
    if state.found.is_none() && state.detecting.is_none() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _ = std::thread::Builder::new().name("gpu-detect".into()).spawn(move || {
            let _ = tx.send(detect());
        });
        state.detecting = Some(rx);
    }
    if let Some(rx) = &state.detecting {
        match rx.try_recv() {
            Ok(g) => {
                state.found = Some(g);
                state.detecting = None;
            }
            Err(_) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }

    let mut changed = false;
    let budget = &mut app.settings.agent.budget;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::MEMORY, &t!("budget.memory"));
        theme::row(ui, &t!("budget.gpu_memory"), |ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut budget.gpu_memory_gb).range(0.0..=256.0).speed(0.25).max_decimals(1).suffix(" GB"))
                .on_hover_text(t!("budget.gpu_memory_hint"))
                .changed();
        });
        theme::row(ui, &t!("budget.ram_memory"), |ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut budget.ram_memory_gb).range(0.0..=1024.0).speed(0.25).max_decimals(1).suffix(" GB"))
                .on_hover_text(t!("budget.ram_memory_hint"))
                .changed();
        });
    });
    ui.add_space(6.0);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.scope(|ui| theme::section(ui, icon::STACK, &t!("budget.at_once"))).response.on_hover_text(t!("budget.at_once_hint"));
        for kind in WorkKind::ALL {
            theme::row(ui, &kind_label(kind), |ui| {
                changed |= ui.add(egui::DragValue::new(budget.limit_mut(kind)).range(1..=16)).changed();
            });
        }
    });
    ui.add_space(6.0);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::GRAPHICS_CARD, &t!("budget.gpus"));
        match &app.agent.budget.found {
            None => {
                ui.spinner();
            }
            Some(found) => {
                if found.gpus.is_empty() {
                    theme::hint(ui, t!("budget.no_gpu"));
                }
                for g in &found.gpus {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&g.name).strong());
                        let kind = if g.discrete { t!("budget.discrete") } else { t!("budget.integrated") };
                        let memory = g.memory.map(|(total, free)| t!("budget.vram", total = gb(total), free = gb(free)).to_string());
                        let parts = [g.vendor.label().to_string(), kind.to_string(), memory.unwrap_or_default()];
                        theme::hint(ui, parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "));
                    });
                }
                if !found.gpus.is_empty() && !found.engine_gpu {
                    theme::hint(ui, t!("budget.cpu_only"));
                }
            }
        }
    });
    if changed {
        app.settings_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::wgpu::DeviceType;

    #[test]
    fn one_entry_per_card_without_software_renderers() {
        let gpus = real_gpus([
            ("AMD Radeon 880M".to_string(), 0x1002, 0x150e, DeviceType::IntegratedGpu),
            ("NVIDIA GeForce RTX 4070".to_string(), 0x10de, 0x2786, DeviceType::DiscreteGpu),
            ("NVIDIA GeForce RTX 4070".to_string(), 0x10de, 0x2786, DeviceType::DiscreteGpu),
            ("Microsoft Basic Render Driver".to_string(), 0x1414, 0x8c, DeviceType::Cpu),
        ]);
        assert_eq!(gpus.len(), 2);
        assert_eq!((gpus[0].vendor, gpus[0].discrete), (Vendor::Nvidia, true), "discrete first");
        assert_eq!(gpus[1].vendor, Vendor::Amd);
    }

    #[test]
    fn memory_from_the_engine_device_of_the_same_card() {
        let mut gpus = real_gpus([
            ("NVIDIA GeForce RTX 4070".to_string(), 0x10de, 0x2786, DeviceType::DiscreteGpu),
            ("AMD Radeon 880M".to_string(), 0x1002, 0x150e, DeviceType::IntegratedGpu),
        ]);
        let nvidia = mediaa_agent::llm::Device {
            name: "Vulkan0".into(),
            description: "NVIDIA GeForce RTX 4070".into(),
            backend: "Vulkan".into(),
            memory_total: 12 << 30,
            memory_free: 6 << 30,
        };
        attach_memory(&mut gpus, &[nvidia]);
        assert_eq!(gpus[0].memory, Some((12 << 30, 6 << 30)));
        assert_eq!(gpus[1].memory, None, "the engine does not use this one");
    }
}
