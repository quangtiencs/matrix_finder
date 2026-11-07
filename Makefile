# Convenience targets. `cargo build --release` still works as usual.
.PHONY: release test icons screenshots app dmg dmg-universal

release:
	cargo build --release

test:
	cargo test

# Regenerate PNG icons, the macOS .icns and the DMG background from the SVGs.
icons:
	cargo test export_assets -- --ignored

# Re-render the README screenshots headlessly (demo data in /tmp/matrix_finder): every scene
# to docs/light and docs/dark, plus docs/showcase.png. SHOTS=panels,palette renders only those scenes.
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
