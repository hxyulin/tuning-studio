mod backend;
mod model;
mod plot;
mod ui;

use clap::Parser;

#[derive(Debug, Default, Parser)]
#[command(about = "Native Live Watch for a tuning-studio session")]
pub struct Options {
    #[arg(long)]
    elf: Option<String>,
    #[arg(long)]
    probe: Option<String>,
    #[arg(long)]
    chip: Option<String>,
    #[arg(long)]
    cli: Option<String>,
    #[arg(long)]
    connect: bool,
}

fn main() -> eframe::Result {
    let args = Options::parse();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1060.0, 720.0])
            .with_min_inner_size([680.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Tuning Studio",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc, args)))),
    )
}
