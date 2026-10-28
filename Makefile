.PHONY: build release install uninstall smoke preview site site-publish clean

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

# Pubblica site/ sul branch gh-pages (https://francescokalle.github.io/lard/,
# e su lard.francescocallegaro.com quando il CNAME è attivo).
site-publish:
	@command -v rsync >/dev/null || { echo "serve rsync"; exit 1; }
	git worktree add -B gh-pages .site-publish gh-pages 2>/dev/null || git worktree add -B gh-pages .site-publish
	rsync -a --delete --exclude CNAME --exclude .git site/ .site-publish/
	cd .site-publish && git add -A && git commit -m "site: publish" || true
	cd .site-publish && git push origin gh-pages
	git worktree remove .site-publish --force

clean:
	cargo clean
