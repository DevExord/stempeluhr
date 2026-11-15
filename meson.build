project(
  'stempeluhr',
  version: '0.1.0',
  meson_version: '>= 1.1',
  license: 'GPL-3.0-or-later',
)

i18n = import('i18n')
gnome = import('gnome')

app_id = 'de.example.Stempeluhr'
gettext_package = meson.project_name()
prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
localedir = prefix / get_option('localedir')
pkgdatadir = datadir / meson.project_name()

subdir('data')
subdir('src')
subdir('po')

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
