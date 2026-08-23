mod audio;
mod cli;
mod cursor;
mod desktop;
mod filesystem;
mod fonts;
mod graphics;
mod linuxulator;
mod paths;
mod portal;
mod remote;
mod runtime;
mod sandbox;
mod startup;
mod state;
mod storage;
mod video;

fn main() -> anyhow::Result<()> {
    cli::run()
}
