mod audio;
mod cli;
mod commands;
mod cursor;
mod desktop;
mod filesystem;
mod fonts;
mod graphics;
mod linuxulator;
mod paths;
mod portal;
mod ps;
mod runtime;
mod sandbox;
mod startup;
mod state;
mod storage;
mod video;

fn main() -> anyhow::Result<()> {
    cli::run()
}
