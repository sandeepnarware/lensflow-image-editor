# Assets

- `logo.png`: the LensFlow logo, 512 x 512 px. Rasterised from the LensFlow web app's
  `frontend/public/logo.svg` (photo-studio-management repository, `origin/main`) with
  `sharp` at 512 px. It is the in-app logo and the window icon.
- `../packaging/icons/`: the installers' icons (`app.ico`, `app.icns`, `16.png` to `512.png`),
  generated from `logo.png` by `scripts/release/make-icons.sh` (or `.ps1`). Regenerate them
  whenever the logo changes; do not edit them by hand.
