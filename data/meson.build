# Blueprint → .ui (Work Area B1)
blueprints = custom_target(
  'blueprints',
  input: files(
    'ui/window.blp',
    'ui/archive_view.blp',
    'ui/progress_row.blp',
    'ui/create_dialog.blp',
  ),
  output: ['window.ui', 'archive_view.ui', 'progress_row.ui', 'create_dialog.ui'],
  command: [
    blueprint_compiler,
    'batch-compile',
    '@OUTDIR@',
    meson.current_source_dir() / 'ui',
    '@INPUT@',
  ],
)

# Bundle the gresource (window.ui from the build dir, style.css from the source
# dir) and install it to pkgdatadir; the app loads it at runtime.
gresources = gnome.compile_resources(
  'septima',
  'resources.gresource.xml',
  gresource_bundle: true,
  dependencies: blueprints,
  source_dir: [meson.current_build_dir(), meson.current_source_dir()],
  install: true,
  install_dir: pkgdatadir,
)

# --- Desktop entry (app id substituted, then translations merged) ---
desktop_conf = configuration_data()
desktop_conf.set('app-id', application_id)
desktop_in = configure_file(
  input: 'io.github.superuser_miguel.Septima.desktop.in.in',
  output: application_id + '.desktop.in',
  configuration: desktop_conf,
)
desktop_file = i18n.merge_file(
  input: desktop_in,
  output: application_id + '.desktop',
  type: 'desktop',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'applications',
)
if desktop_file_validate.found()
  test(
    'validate-desktop',
    desktop_file_validate,
    args: [desktop_file.full_path()],
    depends: desktop_file,
  )
endif

# --- AppStream metainfo (D4) ---
metainfo_conf = configuration_data()
metainfo_conf.set('app-id', application_id)
metainfo_in = configure_file(
  input: 'io.github.superuser_miguel.Septima.metainfo.xml.in.in',
  output: application_id + '.metainfo.xml.in',
  configuration: metainfo_conf,
)
metainfo_file = i18n.merge_file(
  input: metainfo_in,
  output: application_id + '.metainfo.xml',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'metainfo',
)
if appstreamcli.found()
  test(
    'validate-metainfo',
    appstreamcli,
    args: ['validate', '--no-net', '--explain', metainfo_file.full_path()],
    depends: metainfo_file,
  )
endif

# --- App icon (installed under the app id so AppStream + the shell find it) ---
install_data(
  'icons/hicolor/scalable/apps/io.github.superuser_miguel.Septima.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
  rename: application_id + '.svg',
)

# --- GSettings schema (no keys yet; see the file) ---
gschema_conf = configuration_data()
gschema_conf.set('app-id', application_id)
gschema_conf.set('gschema-path', gschema_path)
configure_file(
  input: 'io.github.superuser_miguel.Septima.gschema.xml.in',
  output: application_id + '.gschema.xml',
  configuration: gschema_conf,
  install: true,
  install_dir: datadir / 'glib-2.0' / 'schemas',
)
test(
  'validate-gschema',
  glib_compile_schemas,
  args: ['--strict', '--dry-run', meson.current_build_dir()],
)
