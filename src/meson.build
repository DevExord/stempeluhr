blueprint_compiler = find_program('blueprint-compiler')
cargo = find_program('cargo')

blueprints = custom_target(
  'blueprints',
  input: files(
    'ui/window.blp',
    'ui/connect-dialog.blp',
    'ui/preferences-dialog.blp',
    'ui/shortcuts-dialog.blp',
    'ui/history-page.blp',
    'ui/edit-dialog.blp',
  ),
  output: '.',
  command: [
    blueprint_compiler, 'batch-compile',
    '@OUTPUT@', '@CURRENT_SOURCE_DIR@', '@INPUT@',
  ],
)

resources = gnome.compile_resources(
  'stempeluhr',
  'resources.gresource.xml',
  gresource_bundle: true,
  source_dir: [meson.current_build_dir(), meson.current_source_dir()],
  dependencies: blueprints,
  install: true,
  install_dir: pkgdatadir,
)

cargo_env = environment()
cargo_env.set('STEMPELUHR_APP_ID', app_id)
cargo_env.set('STEMPELUHR_VERSION', meson.project_version())
cargo_env.set('STEMPELUHR_GETTEXT_PACKAGE', gettext_package)
cargo_env.set('STEMPELUHR_LOCALEDIR', localedir)
cargo_env.set('STEMPELUHR_RESOURCES_FILE', pkgdatadir / 'stempeluhr.gresource')

profile = get_option('buildtype') == 'release' or get_option('buildtype') == 'debugoptimized' ? 'release' : 'debug'
mode = get_option('offline') ? 'offline' : 'online'
cargo_sources = files(
  '../Cargo.toml', '../Cargo.lock', '../core/Cargo.toml',
)

stempeluhr_bin = custom_target(
  'stempeluhr-bin',
  build_by_default: true,
  build_always_stale: true,
  output: 'stempeluhr',
  console: true,
  install: true,
  install_dir: bindir,
  env: cargo_env,
  command: [
    meson.project_source_root() / 'build-aux/cargo-build.sh',
    cargo.full_path(),
    meson.project_source_root() / 'Cargo.toml',
    meson.project_build_root() / 'cargo-target',
    profile, mode, '@OUTPUT@',
  ],
)

# Run the app straight from the build directory with `meson devenv`.
devenv = environment()
devenv.set('STEMPELUHR_RESOURCES_FILE', meson.current_build_dir() / 'stempeluhr.gresource')
meson.add_devenv(devenv)

test(
  'core (client, state machine, scenarios)',
  cargo,
  args: ['test', '--manifest-path', meson.project_source_root() / 'Cargo.toml', '-p', 'stempeluhr-core'],
  timeout: 600,
)
