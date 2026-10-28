# Style catalog

`manifest.json` lists the font, icon theme, and cursor theme choices shipped by
nix-settings-gui. Every entry must name a package that exists in the Nixpkgs
revision used by the template. Selecting an entry writes both the value and the
package expression into the configuration.

Preview files are PNGs stored in `previews/`. Font entries use one preview file;
icon and cursor entries use an array of preview files rendered as a grid. The
files are embedded into the binary by `build.rs`.

Run `scripts/generate-style-previews.sh` to regenerate the previews. Run it in an
environment where the catalog font, icon, and cursor packages are installed, so
Pango renders the real families. The font renderer uses Pango, the icon renderer
fetches the Papirus source SVGs and uses `rsvg-convert`, and the cursor renderer
decodes Xcursor files from an installed Bibata theme. The script does not modify `manifest.json`.
