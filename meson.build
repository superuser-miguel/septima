project(
  'septima',
  version: '0.1.0',
  meson_version: '>= 0.60.0',
  license: 'GPL-3.0-or-later',
)

i18n = import('i18n')
gnome = import('gnome')

base_id = 'io.github.superuser_miguel.Septima'
gettext_package = 'septima'

# Runtime dependencies — satisfied by the host or the GNOME Flatpak SDK. Not
# linked here (cargo does the linking); declared so misconfigured build
# environments fail early with a clear message.
dependency('glib-2.0', version: '>= 2.66')
dependency('gtk4', version: '>= 4.10')
dependency('libadwaita-1', version: '>= 1.4')

# Tools
cargo = find_program('cargo', required: true)
blueprint_compiler = find_program('blueprint-compiler', required: true)
glib_compile_schemas = find_program('glib-compile-schemas', required: true)
desktop_file_validate = find_program('desktop-file-validate', required: false)
appstreamcli = find_program('appstreamcli', required: false)

# Profile → identity. The gresource *prefix* always uses the base id (stable),
# only the app id gains the .Devel suffix, so Flatpak and dev builds coexist.
profile = get_option('profile')
if profile == 'development'
  application_id = base_id + '.Devel'
  profile_name = 'Devel'
else
  application_id = base_id
  profile_name = 'Default'
endif
gschema_path = '/' + '/'.join(application_id.split('.')) + '/'

# Install locations
prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
localedir = prefix / get_option('localedir')
pkgdatadir = datadir / meson.project_name()

subdir('data')
subdir('po')

# --- Build the Rust binary via cargo (config flows through the environment) ---
# CARGO_HOME is intentionally NOT set here: on the host cargo uses the default
# registry; under Flatpak the manifest sets it to the vendored dir (offline).
cargo_env = environment()
cargo_env.set('SEPTIMA_APP_ID', application_id)
cargo_env.set('SEPTIMA_PROFILE', profile_name)
cargo_env.set('SEPTIMA_LOCALEDIR', localedir)
cargo_env.set('SEPTIMA_PKGDATADIR', pkgdatadir)

if profile == 'development'
  cargo_profile_dir = 'debug'
  cargo_extra = []
else
  cargo_profile_dir = 'release'
  cargo_extra = ['--release']
endif

# Flatpak builds vendor crates (build-aux/cargo-sources.json) and build offline.
if get_option('offline')
  cargo_extra += ['--offline']
endif
custom_target(
  'cargo-build',
  build_always_stale: true,
  build_by_default: true,
  output: 'septima',
  console: true,
  install: true,
  install_dir: bindir,
  env: cargo_env,
  depends: gresources,
  command: [
    find_program('build-aux/cargo-build.sh'),
    meson.project_source_root() / 'Cargo.toml',
    meson.project_build_root() / 'target',
    cargo_profile_dir,
    '@OUTPUT@',
    cargo_extra,
  ],
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)

summary(
  {
    'Profile': profile,
    'App ID': application_id,
    'Prefix': prefix,
  },
  section: 'Configuration',
)
