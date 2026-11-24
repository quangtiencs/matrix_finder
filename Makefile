# Convenience targets. `cargo build --release` still works as usual.
.PHONY: release test fmt fmt-check icons screenshots app dmg dmg-universal linux-packages licenses

release:
	cargo build --release

test:
	cargo test

# Format the code (rustfmt.toml); fmt-check fails on unformatted code instead.
fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

# Regenerate PNG icons, the macOS .icns and the DMG background from the SVGs.
icons:
	cargo test export_assets -- --ignored

# Re-render the README screenshots headlessly (demo data in /tmp/matrix_finder): every scene
# to docs/light and docs/dark, plus docs/showcase.png and docs/banner.png. SHOTS=panels,palette renders only those scenes.
screenshots:
	MATRIX_FINDER_SHOTS=$(SHOTS) cargo test export_screenshots -- --ignored --nocapture

# macOS: target/macos/Matrix Finder.app
app:
	scripts/macos/build-dmg.sh --app-only

# macOS: target/macos/MatrixFinder-<version>-macos-<arch>.dmg (open it, drag the app to Applications)
dmg:
	scripts/macos/build-dmg.sh

dmg-universal:
	scripts/macos/build-dmg.sh --universal

# Linux: target/linux/matrix-finder_<version>-1_<arch>.deb and matrix-finder-<version>-1.<arch>.rpm,
# built in Docker on Ubuntu 22.04. ARCH=amd64 or arm64 (default: this machine's).
linux-packages:
	scripts/linux/build-packages.sh $(if $(ARCH),--arch $(ARCH))

# THIRD-PARTY-LICENSES.md: the licenses of every crate, C library, font and icon built in.
# Run after changing dependencies (CI fails while it is out of date).
licenses:
	python3 scripts/third-party-licenses.py
