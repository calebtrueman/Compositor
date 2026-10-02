# Compositor

Adobe Photoshop costs too much and tools like GIMP don’t feel familiar enough for me to stay in flow. That’s why I built Compositor.

The goal was to create a full-featured image editor that is completely free and open source. I used to use Photoshop for compositing and post-processing, so Compositor is built around that workflow - with the tools needed to create a pixel-perfect final image.

Because it’s open source, you can download the Xcode project and add, remove, or modify any feature to fit your workflow.

## Installation

### Download
Get Compositor from [robbietilton.com/compositor](https://robbietilton.com/compositor), or download the latest release directly from [GitHub Releases](https://github.com/robbietilton/Compositor/releases/latest).

### Homebrew

```sh
brew install --cask robbietilton-compositor
```

### Linux

This fork adds a native Linux edition — see [Linux](#linux) below for packages for every major distribution.

## Features

### Layers
- Layers and folders, with opacity and Photoshop's full set of blend modes in its order — a folder's opacity dims everything inside it
- Layer masks: paint, fill, invert, blur and feather them anywhere on the canvas, past the layer's own pixels; link or unlink them to transform a mask on its own
- Clipping masks and folder masks
- Adjustment layers: Hue/Saturation, Levels, Curves, Exposure, Gradient Map, Grain, Black & White, Color Balance, Invert, Gaussian Blur, Motion Blur and Noise
- Layer effects: Stroke, Drop Shadow, Color Overlay, Inner Shadow, Outer Glow and Inner Glow, rendered on the GPU and editable at any time
- Merge Down, Merge Layers and Merge Group (⌘E)
- Duplicate, rename inline, reorder and nest by drag and drop; Option-drag to duplicate; a right-click menu in the Layers panel
- Copy and paste whole layers and folders (⌘C/⌘V with no selection), within a project or between projects, or drag them between projects

### Transform
- Non-destructive move, scale, rotate and flip — images keep their full resolution however small you make them
- Free distort (⌘-drag a handle), with Shift to lock to an axis
- Transform several layers, or a whole folder, together
- Snapping to canvas and layer edges and centers, with guides
- Exact values for position, size, scale and angle, stepped with the arrow keys
- Flip Layer and Flip Canvas, horizontal and vertical

### Selections
- Rectangle and Ellipse Marquee, Freehand and Polygonal Lasso, and the Magic tool — Wand selects by color, Object traces whatever you click (Tab switches)
- Select Subject, and Expand, Contract and Feather on any selection
- Add to and subtract from selections, move the outline, or move and duplicate the pixels inside
- Load a layer's pixels or a mask as a selection
- Content-Aware Fill, which can also extend an image past its edges

### Painting and retouching
- Brush with size, hardness, opacity and smoothing, in Paint or Erase mode (B and E), and Shift for straight lines
- Spot Healing Brush (content-aware)
- Clone Stamp, aligned or not, sampling one layer or all of them
- Blur tool, on pixels or masks
- Gradient tool and Shape tool (rectangles, rounded rectangles, ellipses and lines), which stay editable rather than being rasterized
- Type tool (T): inline multiline editing in draggable, resizable paragraph boxes; font, size, color, alignment and spacing in the tool header; transform text and use it as a clipping mask
- Eyedropper and a full color picker

### Adjustments and filters
- Camera Raw filter: light, color, curves, color mixer, color grading, detail, optics and geometry, in a panel beside the canvas
- Levels (with Auto), Curves, Hue/Saturation, Exposure, Gradient Map, Grain, Black & White, Color Balance and Invert
- Gaussian Blur and Motion Blur that spread past a layer's edges
- Add Noise, Vignette, Bloom / Glow, Tonal Contrast, Lens Correction and Remove Background
- Live previews, limited to the selection when there is one

### Canvas and files
- Multiple projects in tabs
- Rulers (⌘R), guides dragged from them, a layout grid with adjustable spacing and subdivisions, and Snap To for guides, grid, layers and document bounds
- Crop with snapping, ratios including 3:4 and 9:16, and Option for symmetric cropping; with a selection, the crop starts at it
- Canvas Size, Image Size and Trim
- Sharp high-quality downsampling when zoomed out, and a pixel grid when zoomed in
- Import JPEG, PNG, HEIC, TIFF, SVG, camera RAW (with a develop step first) and Photoshop PSD and PSB (8-bit RGB; not CMYK). Photoshop folders, masks, blend modes, fill rectangles/ellipses, and simple horizontal text stay editable; other vectors and vertical text become pixels. A conversion report is shown before anything is applied.
- Large documents: the memory budget scales with your Mac, and a Photoshop file too big to open has its layers cropped to the canvas instead
- Export JPEG with a live preview (⇧⌥⌘S); Copy Merged
- Keep working while a project saves
- Photoshop-style keyboard shortcuts throughout, remappable in Edit > Keyboard Shortcuts
- Drag a number's label to scrub its value, as in Photoshop
- Automatic updates, signed and notarized

### Works with AI agents
- AI agents and scripts can build and edit projects directly: a `.comp` is a folder of PNG layers and a manifest, and an open project updates live as it's written. See [Writing Compositor projects](docs/writing-comp-files.md)

## Requirements

- macOS 26.0 or later on a Mac with Apple silicon
- Xcode 26 or later (to build from source)

## Building

Open `Compositor.xcodeproj` and run the **Compositor** scheme.

## Releasing

`scripts/release.sh` builds a Release version, signs it with Developer ID, notarizes and staples it, and packages it into `dist/Compositor-<version>.dmg`.

It needs, all kept outside this repository:

- a **Developer ID Application** certificate in the login keychain
- notarization credentials saved with `xcrun notarytool store-credentials "compositor-notary" …`
- [`create-dmg`](https://github.com/create-dmg/create-dmg) (`brew install create-dmg`)

## Linux

This fork ([calebtrueman/Compositor](https://github.com/calebtrueman/Compositor)) adds Compositor for Linux, a native rewrite in Rust that lives in [`linux/`](linux/) ([architecture](linux/ARCHITECTURE.md)). It runs on X11 and Wayland, on x86_64 and ARM64 (aarch64), and needs only OpenGL — Mesa or your GPU vendor's driver.

It opens and saves the same `.comp` projects as the macOS app, so a project can move between a Mac and a Linux machine and keep its layers, folders, masks, blend modes, adjustment layers and transforms. It also opens PNG, JPEG, TIFF, WebP, BMP, GIF and Photoshop PSD files. The Linux edition is a newer port and does not yet have every feature listed above for the macOS app.

### Install

Download the file for your distribution from the [latest release](https://github.com/calebtrueman/Compositor/releases/latest). In the commands below, replace `X.Y.Z` with the version, and `x86_64`/`amd64` with `aarch64`/`arm64` on an ARM machine. Every format installs the `compositor` command and a **Compositor** entry in your app menu.

**Debian, Ubuntu, Linux Mint, Pop!_OS, elementary OS, Zorin OS** (`.deb`)

```sh
sudo apt install ./compositor-image-editor_X.Y.Z-1_amd64.deb
```

**Fedora, RHEL, Rocky Linux, AlmaLinux** (`.rpm`)

```sh
sudo dnf install ./compositor-image-editor-X.Y.Z-1.x86_64.rpm
```

**openSUSE Tumbleweed and Leap** (`.rpm`)

```sh
sudo zypper install --allow-unsigned-rpm ./compositor-image-editor-X.Y.Z-1.x86_64.rpm
```

**Arch Linux, EndeavourOS, Manjaro** (`.pkg.tar.zst`, x86_64)

```sh
sudo pacman -U ./compositor-image-editor-bin-X.Y.Z-1-x86_64.pkg.tar.zst
```

A source PKGBUILD for the AUR (`compositor-image-editor`) is in [`linux/packaging/arch/`](linux/packaging/arch/); on ARM, build it with `makepkg -si` there.

**Flatpak** (any distribution)

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./Compositor-X.Y.Z-x86_64.flatpak
flatpak run io.github.calebtrueman.Compositor
```

The Flatpak can read and write your home folder and removable drives.

**AppImage** (any distribution, nothing to install)

```sh
chmod +x Compositor-X.Y.Z-x86_64.AppImage
./Compositor-X.Y.Z-x86_64.AppImage
```

If your system has no FUSE (some containers and minimal installs), run it with `APPIMAGE_EXTRACT_AND_RUN=1 ./Compositor-X.Y.Z-x86_64.AppImage`.

**Anything else** (`.tar.gz`)

```sh
tar xf compositor-image-editor-X.Y.Z-linux-x86_64.tar.gz
cd compositor-image-editor-X.Y.Z-linux-x86_64
./install.sh            # into ~/.local; or `sudo ./install.sh` for /usr/local
```

`./install.sh --uninstall` removes it again. You can also run `./bin/compositor` without installing.

To check a download, put `SHA256SUMS` from the release beside it and run `sha256sum -c SHA256SUMS --ignore-missing`.

The packaged builds need glibc 2.31 or newer: Debian 11, Ubuntu 20.04, Fedora 32, RHEL 9, openSUSE Leap 15.3 or anything later.

### Build from source

Install Rust 1.85 or newer from [rustup.rs](https://rustup.rs), plus a C toolchain and the X11/Wayland/OpenGL development files:

| Distribution | Command |
| --- | --- |
| Debian, Ubuntu and derivatives | `sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev libegl1-mesa-dev` |
| Fedora, RHEL-family | `sudo dnf install gcc pkgconf-pkg-config libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libX11-devel libXcursor-devel libXrandr-devel libXi-devel mesa-libGL-devel mesa-libEGL-devel` |
| openSUSE | `sudo zypper install gcc pkgconf libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libX11-devel libXcursor-devel libXrandr-devel libXi-devel Mesa-libGL-devel Mesa-libEGL-devel` |
| Arch Linux | `sudo pacman -S --needed base-devel rust libxkbcommon libxkbcommon-x11 wayland libx11 libxcursor libxrandr libxi libglvnd` |

Then:

```sh
cd linux
cargo build --release
./target/release/compositor
```

To install the build system-wide with its menu entry, icons and `.comp` file type:

```sh
sudo bash packaging/scripts/stage.sh target/release/compositor "" /usr/local
```

How the Linux packages are built and released is described in [`linux/packaging/README.md`](linux/packaging/README.md).

## License

MIT — see [LICENSE](LICENSE).
