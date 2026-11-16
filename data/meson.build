desktop_conf = configuration_data()
desktop_conf.set('app_id', app_id)
desktop_in = configure_file(
  input: 'digital.fischers.stempeluhr.desktop.in.in',
  output: app_id + '.desktop.in',
  configuration: desktop_conf,
)
desktop_file = i18n.merge_file(
  input: desktop_in,
  output: app_id + '.desktop',
  type: 'desktop',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'applications',
)
desktop_validate = find_program('desktop-file-validate', required: false)
if desktop_validate.found()
  test('Validate desktop file', desktop_validate, args: [desktop_file])
endif

metainfo_file = i18n.merge_file(
  input: 'digital.fischers.stempeluhr.metainfo.xml.in',
  output: app_id + '.metainfo.xml',
  type: 'xml',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'metainfo',
)
appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test('Validate metainfo', appstreamcli, args: ['validate', '--no-net', '--explain', metainfo_file])
endif

install_data(
  'digital.fischers.stempeluhr.gschema.xml',
  install_dir: datadir / 'glib-2.0' / 'schemas',
)
compile_schemas = find_program('glib-compile-schemas', required: false)
if compile_schemas.found()
  test(
    'Validate schema file',
    compile_schemas,
    args: ['--strict', '--dry-run', meson.current_source_dir()],
  )
endif
gnome.compile_schemas(build_by_default: true)

service_conf = configuration_data()
service_conf.set('app_id', app_id)
service_conf.set('bindir', bindir)
configure_file(
  input: 'digital.fischers.stempeluhr.service.in',
  output: app_id + '.service',
  configuration: service_conf,
  install_dir: datadir / 'dbus-1' / 'services',
)

install_data(
  'icons/hicolor/scalable/apps/digital.fischers.stempeluhr.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
)
install_data(
  'icons/hicolor/symbolic/apps/digital.fischers.stempeluhr-symbolic.svg',
  install_dir: datadir / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
