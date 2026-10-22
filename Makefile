.PHONY: build release install uninstall smoke preview clean

build:
	cargo build --locked --bin lard

release:
	cargo build --release --locked --bin lard

install:
	./install.sh

uninstall:
	./install.sh --uninstall

smoke:
	cargo run -p lard-render --example headless_smoke

preview:
	cargo run -p lard-shell --example preview

clean:
	cargo clean
