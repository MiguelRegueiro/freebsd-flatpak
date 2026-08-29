#include <gio/gio.h>
#include <glib-unix.h>
#include <errno.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include <stdio.h>
#include <stdarg.h>
#include <string.h>

#define NM_NAME "org.freedesktop.NetworkManager"
#define NM_PATH "/org/freedesktop/NetworkManager"
#define SETTINGS_PATH "/org/freedesktop/NetworkManager/Settings"
#define AGENT_PATH "/org/freedesktop/NetworkManager/AgentManager"
#define OBJECT_MANAGER_PATH "/org/freedesktop"
#define ACTIVE_PATH NM_PATH "/ActiveConnection"
#define DEVICE_PATH NM_PATH "/Devices"
#define NETWORK_HELPER "/usr/local/libexec/freebsd-flatpak/network-manager-privileged"

static FILE *trace_output;
typedef struct {
  gchar *path;
  GVariant *settings;
  guint registration_id;
} CompatConnection;

typedef struct {
  gchar *path;
  gchar *connection_path;
  gchar *device_path;
  gchar *interface_name;
  gchar *token;
  gchar *type;
  guint registration_id;
  guint device_registration_id;
  gboolean active;
  gboolean default_ipv4;
  gboolean default_ipv6;
} CompatActiveConnection;

static GDBusConnection *service_connection;
static GDBusNodeInfo *service_node;
static GPtrArray *connections;
static GPtrArray *active_connections;
static guint next_connection_id;
static guint next_active_connection_id;
static gchar *network_helper = NULL;

static void trace_log(const char *format, ...);
static gboolean deactivate_active(CompatActiveConnection *entry, GError **error);
static gboolean activate_connection(CompatConnection *connection, gchar **active_path, GError **error);
static GVariant *active_paths(void);
static GVariant *device_paths(void);
static GVariant *active_properties(CompatActiveConnection *entry);
static GVariant *device_properties(CompatActiveConnection *entry);

static const char *introspection_xml =
    "<node>"
    " <interface name='org.freedesktop.DBus.ObjectManager'>"
    "  <method name='GetManagedObjects'><arg type='a{oa{sa{sv}}}' direction='out'/></method>"
    "  <signal name='InterfacesAdded'><arg type='o'/><arg type='a{sa{sv}}'/></signal>"
    "  <signal name='InterfacesRemoved'><arg type='o'/><arg type='as'/></signal>"
    " </interface>"
    " <interface name='org.freedesktop.NetworkManager'>"
    "  <method name='GetDevices'><arg type='ao' direction='out'/></method>"
    "  <method name='GetAllDevices'><arg type='ao' direction='out'/></method>"
    "  <method name='CheckConnectivity'><arg type='u' direction='out'/></method>"
    "  <method name='GetPermissions'><arg type='a{ss}' direction='out'/></method>"
    "  <method name='Enable'><arg type='b' direction='in'/></method>"
    "  <method name='ActivateConnection'><arg type='o' direction='in'/>"
    "   <arg type='o' direction='in'/><arg type='o' direction='in'/>"
    "   <arg type='o' direction='out'/></method>"
    "  <method name='AddAndActivateConnection2'><arg type='a{sa{sv}}' direction='in'/>"
    "   <arg type='o' direction='in'/><arg type='o' direction='in'/>"
    "   <arg type='a{sv}' direction='in'/><arg type='o' direction='out'/>"
    "   <arg type='a{sv}' direction='out'/></method>"
    "  <method name='DeactivateConnection'><arg type='o' direction='in'/></method>"
    "  <property name='Version' type='s' access='read'/>"
    "  <property name='VersionInfo' type='au' access='read'/>"
    "  <property name='State' type='u' access='read'/>"
    "  <property name='Startup' type='b' access='read'/>"
    "  <property name='NetworkingEnabled' type='b' access='read'/>"
    "  <property name='WirelessEnabled' type='b' access='read'/>"
    "  <property name='WirelessHardwareEnabled' type='b' access='read'/>"
    "  <property name='WwanEnabled' type='b' access='read'/>"
    "  <property name='WwanHardwareEnabled' type='b' access='read'/>"
    "  <property name='Connectivity' type='u' access='read'/>"
    "  <property name='ConnectivityCheckAvailable' type='b' access='read'/>"
    "  <property name='ConnectivityCheckEnabled' type='b' access='readwrite'/>"
    "  <property name='ConnectivityCheckUri' type='s' access='read'/>"
    "  <property name='ActiveConnections' type='ao' access='read'/>"
    "  <property name='PrimaryConnection' type='o' access='read'/>"
    "  <property name='PrimaryConnectionType' type='s' access='read'/>"
    "  <property name='ActivatingConnection' type='o' access='read'/>"
    "  <property name='Metered' type='u' access='read'/>"
    "  <property name='Devices' type='ao' access='read'/>"
    "  <property name='AllDevices' type='ao' access='read'/>"
    "  <property name='Capabilities' type='au' access='read'/>"
    " </interface>"
    " <interface name='org.freedesktop.NetworkManager.Settings'>"
    "  <method name='ListConnections'><arg type='ao' direction='out'/></method>"
    "  <method name='AddConnection'><arg type='a{sa{sv}}' direction='in'/>"
    "   <arg type='o' direction='out'/></method>"
    "  <method name='AddConnectionUnsaved'><arg type='a{sa{sv}}' direction='in'/>"
    "   <arg type='o' direction='out'/></method>"
    "  <method name='AddConnection2'><arg type='a{sa{sv}}' direction='in'/>"
    "   <arg type='u' direction='in'/><arg type='a{sv}' direction='in'/>"
    "   <arg type='o' direction='out'/><arg type='a{sv}' direction='out'/></method>"
    "  <method name='SaveHostname'><arg type='s' direction='in'/></method>"
    "  <property name='Connections' type='ao' access='read'/>"
    "  <property name='Hostname' type='s' access='read'/>"
    "  <property name='CanModify' type='b' access='read'/>"
    "  <property name='VersionId' type='t' access='read'/>"
    "  <signal name='NewConnection'><arg type='o'/></signal>"
    "  <signal name='ConnectionRemoved'><arg type='o'/></signal>"
    " </interface>"
    " <interface name='org.freedesktop.NetworkManager.Settings.Connection'>"
    "  <method name='GetSettings'><arg type='a{sa{sv}}' direction='out'/></method>"
    "  <method name='GetSecrets'><arg type='s' direction='in'/><arg type='a{sa{sv}}' direction='out'/></method>"
    "  <method name='Delete'/>"
    "  <property name='Unsaved' type='b' access='read'/>"
    "  <property name='Filename' type='s' access='read'/>"
    "  <property name='Flags' type='u' access='read'/>"
    "  <signal name='Updated'/>"
    "  <signal name='Removed'/>"
    " </interface>"
    " <interface name='org.freedesktop.NetworkManager.Connection.Active'>"
    "  <property name='Connection' type='o' access='read'/>"
    "  <property name='SpecificObject' type='o' access='read'/>"
    "  <property name='Id' type='s' access='read'/>"
    "  <property name='Uuid' type='s' access='read'/>"
    "  <property name='Type' type='s' access='read'/>"
    "  <property name='Devices' type='ao' access='read'/>"
    "  <property name='State' type='u' access='read'/>"
    "  <property name='Default' type='b' access='read'/>"
    "  <property name='Default6' type='b' access='read'/>"
    "  <property name='Vpn' type='b' access='read'/>"
    "  <property name='Master' type='o' access='read'/>"
    " </interface>"
    " <interface name='org.freedesktop.NetworkManager.Device'>"
    "  <property name='Interface' type='s' access='read'/>"
    "  <property name='IpInterface' type='s' access='read'/>"
    "  <property name='DeviceType' type='u' access='read'/>"
    "  <property name='State' type='u' access='read'/>"
    "  <property name='ActiveConnection' type='o' access='read'/>"
    " </interface>"
    " <interface name='org.freedesktop.NetworkManager.AgentManager'>"
    "  <method name='Register'><arg type='s' direction='in'/></method>"
    "  <method name='RegisterWithCapabilities'><arg type='s' direction='in'/>"
    "   <arg type='u' direction='in'/></method>"
    "  <method name='Unregister'/><method name='Enable'><arg type='b' direction='in'/></method>"
    " </interface>"
    "</node>";

static GVariant *empty_array(const char *type) {
  GVariantBuilder builder;
  g_variant_builder_init(&builder, G_VARIANT_TYPE(type));
  return g_variant_builder_end(&builder);
}
static void connection_free(gpointer data) {
  CompatConnection *entry = data;
  if (service_connection && entry->registration_id)
    g_dbus_connection_unregister_object(service_connection, entry->registration_id);
  g_clear_pointer(&entry->settings, g_variant_unref);
  g_free(entry->path);
  g_free(entry);
}

static GVariant *connection_paths(void) {
  GVariantBuilder paths;
  g_variant_builder_init(&paths, G_VARIANT_TYPE("ao"));
  for (guint i = 0; connections && i < connections->len; i++) {
    CompatConnection *entry = g_ptr_array_index(connections, i);
    g_variant_builder_add(&paths, "o", entry->path);
  }
  return g_variant_builder_end(&paths);
}

static GVariant *connection_properties(void) {
  GVariantBuilder properties;
  g_variant_builder_init(&properties, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(&properties, "{sv}", "Unsaved", g_variant_new_boolean(TRUE));
  g_variant_builder_add(&properties, "{sv}", "Filename", g_variant_new_string(""));
  g_variant_builder_add(&properties, "{sv}", "Flags", g_variant_new_uint32(1u));
  return g_variant_builder_end(&properties);
}

static GVariant *connection_interfaces(void) {
  GVariantBuilder interfaces;
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("a{sa{sv}}"));
  g_variant_builder_add(&interfaces, "{s@a{sv}}",
                        NM_NAME ".Settings.Connection", connection_properties());
  return g_variant_builder_end(&interfaces);
}

static void emit_interfaces_added(const char *path) {
  g_dbus_connection_emit_signal(
      service_connection, NULL, OBJECT_MANAGER_PATH,
      "org.freedesktop.DBus.ObjectManager", "InterfacesAdded",
      g_variant_new("(o@a{sa{sv}})", path, connection_interfaces()), NULL);
}

static void emit_interfaces_removed(const char *path) {
  GVariantBuilder interfaces;
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("as"));
  g_variant_builder_add(&interfaces, "s", NM_NAME ".Settings.Connection");
  g_dbus_connection_emit_signal(
      service_connection, NULL, OBJECT_MANAGER_PATH,
      "org.freedesktop.DBus.ObjectManager", "InterfacesRemoved",
      g_variant_new("(o@as)", path, g_variant_builder_end(&interfaces)), NULL);
}

static gboolean is_secret_property(const char *name) {
  return g_str_equal(name, "password") || g_str_equal(name, "private-key") ||
         g_str_equal(name, "preshared-key") || g_str_equal(name, "psk") ||
         g_str_has_suffix(name, "-password") || g_str_has_suffix(name, "-secret");
}

/* Peer secrets are nested in wireguard.peers rather than direct setting keys. */
static GVariant *public_wireguard_peers(GVariant *boxed_peers) {
  if (!g_variant_is_of_type(boxed_peers, G_VARIANT_TYPE_VARIANT)) return boxed_peers;
  GVariant *peers = g_variant_get_variant(boxed_peers);
  g_variant_unref(boxed_peers);
  if (!g_variant_is_of_type(peers, G_VARIANT_TYPE("aa{sv}"))) {
    GVariant *result = g_variant_new_variant(peers);
    g_variant_unref(peers);
    return result;
  }
  GVariantBuilder public_peers;
  g_variant_builder_init(&public_peers, G_VARIANT_TYPE("aa{sv}"));
  GVariantIter peer_iter;
  GVariant *peer;
  g_variant_iter_init(&peer_iter, peers);
  while ((peer = g_variant_iter_next_value(&peer_iter))) {
    GVariantBuilder public_peer;
    GVariantIter fields;
    const char *field_name;
    GVariant *field;
    g_variant_builder_init(&public_peer, G_VARIANT_TYPE("a{sv}"));
    g_variant_iter_init(&fields, peer);
    while (g_variant_iter_next(&fields, "{&s@v}", &field_name, &field)) {
      if (!is_secret_property(field_name))
        g_variant_builder_add(&public_peer, "{s@v}", field_name, field);
      else
        g_variant_unref(field);
    }
    g_variant_builder_add(&public_peers, "@a{sv}", g_variant_builder_end(&public_peer));
    g_variant_unref(peer);
  }
  g_variant_unref(peers);
  return g_variant_new_variant(g_variant_builder_end(&public_peers));
}
static GVariant *secret_wireguard_peers(GVariant *boxed_peers) {
  if (!g_variant_is_of_type(boxed_peers, G_VARIANT_TYPE_VARIANT)) return NULL;
  GVariant *peers = g_variant_get_variant(boxed_peers);
  if (!g_variant_is_of_type(peers, G_VARIANT_TYPE("aa{sv}"))) { g_variant_unref(peers); return NULL; }
  GVariantBuilder secret_peers;
  gboolean any_peer = FALSE;
  g_variant_builder_init(&secret_peers, G_VARIANT_TYPE("aa{sv}"));
  GVariantIter peer_iter;
  GVariant *peer;
  g_variant_iter_init(&peer_iter, peers);
  while ((peer = g_variant_iter_next_value(&peer_iter))) {
    GVariantBuilder secret_peer;
    GVariantIter fields;
    const char *field_name;
    GVariant *field;
    GVariant *public_key = NULL;
    gboolean has_secret = FALSE;
    g_variant_builder_init(&secret_peer, G_VARIANT_TYPE("a{sv}"));
    g_variant_iter_init(&fields, peer);
    while (g_variant_iter_next(&fields, "{&s@v}", &field_name, &field)) {
      if (g_str_equal(field_name, "public-key")) public_key = field;
      else if (is_secret_property(field_name)) { g_variant_builder_add(&secret_peer, "{s@v}", field_name, field); has_secret = TRUE; }
      else g_variant_unref(field);
    }
    if (has_secret) {
      if (public_key) g_variant_builder_add(&secret_peer, "{s@v}", "public-key", public_key);
      g_variant_builder_add(&secret_peers, "@a{sv}", g_variant_builder_end(&secret_peer));
      any_peer = TRUE;
    } else {
      if (public_key) g_variant_unref(public_key);
      g_variant_builder_clear(&secret_peer);
    }
    g_variant_unref(peer);
  }
  g_variant_unref(peers);
  return any_peer ? g_variant_builder_end(&secret_peers) : NULL;
}
static GVariant *secret_settings(GVariant *settings, const char *requested_setting) {
  GVariant *section = g_variant_lookup_value(settings, requested_setting, G_VARIANT_TYPE("a{sv}"));
  GVariantBuilder result;
  g_variant_builder_init(&result, G_VARIANT_TYPE("a{sa{sv}}"));
  if (!section) return g_variant_builder_end(&result);
  GVariantBuilder secret_section;
  gboolean any_secret = FALSE;
  GVariantIter fields;
  const char *field_name;
  GVariant *field;
  g_variant_builder_init(&secret_section, G_VARIANT_TYPE("a{sv}"));
  g_variant_iter_init(&fields, section);
  while (g_variant_iter_next(&fields, "{&s@v}", &field_name, &field)) {
    if (is_secret_property(field_name)) { g_variant_builder_add(&secret_section, "{s@v}", field_name, field); any_secret = TRUE; }
    else if (g_str_equal(requested_setting, "wireguard") && g_str_equal(field_name, "peers")) {
      GVariant *peers = secret_wireguard_peers(field);
      if (peers) { g_variant_builder_add(&secret_section, "{s@v}", field_name, g_variant_new_variant(peers)); any_secret = TRUE; }
    } else g_variant_unref(field);
  }
  if (any_secret) g_variant_builder_add(&result, "{s@a{sv}}", requested_setting, g_variant_builder_end(&secret_section));
  else g_variant_builder_clear(&secret_section);
  g_variant_unref(section);
  return g_variant_builder_end(&result);
}
static GVariant *public_settings(GVariant *settings) {
  GVariantBuilder public_settings;
  GVariantIter sections;
  const char *section_name;
  GVariant *section;
  g_variant_builder_init(&public_settings, G_VARIANT_TYPE("a{sa{sv}}"));
  g_variant_iter_init(&sections, settings);
  while (g_variant_iter_next(&sections, "{&s@a{sv}}", &section_name, &section)) {
    GVariantBuilder public_section;
    GVariantIter fields;
    const char *field_name;
    GVariant *field;
    g_variant_builder_init(&public_section, G_VARIANT_TYPE("a{sv}"));
    g_variant_iter_init(&fields, section);
    while (g_variant_iter_next(&fields, "{&s@v}", &field_name, &field)) {
      if (!is_secret_property(field_name)) {
        if (g_str_equal(section_name, "wireguard") && g_str_equal(field_name, "peers"))
          g_variant_builder_add(&public_section, "{s@v}", field_name, public_wireguard_peers(field));
        else
          g_variant_builder_add(&public_section, "{s@v}", field_name, field);
      } else
        g_variant_unref(field);
    }
    g_variant_builder_add(&public_settings, "{s@a{sv}}", section_name,
                          g_variant_builder_end(&public_section));
    g_variant_unref(section);
  }
  return g_variant_builder_end(&public_settings);
}

static void emit_settings_changed(void) {
  GVariantBuilder changes;
  g_variant_builder_init(&changes, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(&changes, "{sv}", "Connections", connection_paths());
  g_variant_builder_add(&changes, "{sv}", "VersionId", g_variant_new_uint64(next_connection_id));
  g_dbus_connection_emit_signal(service_connection, NULL, SETTINGS_PATH,
      "org.freedesktop.DBus.Properties", "PropertiesChanged",
      g_variant_new("(s@a{sv}@as)", NM_NAME ".Settings",
                    g_variant_builder_end(&changes), empty_array("as")), NULL);
}

static void emit_connection_added(const char *path) {
  g_dbus_connection_emit_signal(service_connection, NULL, SETTINGS_PATH,
      NM_NAME ".Settings", "NewConnection", g_variant_new("(o)", path), NULL);
  emit_settings_changed();
}

static void emit_connection_removed(const char *path) {
  g_dbus_connection_emit_signal(service_connection, NULL, path,
      NM_NAME ".Settings.Connection", "Removed", NULL, NULL);
  g_dbus_connection_emit_signal(service_connection, NULL, SETTINGS_PATH,
      NM_NAME ".Settings", "ConnectionRemoved", g_variant_new("(o)", path), NULL);
  emit_settings_changed();
}
static void connection_method_call(
    GDBusConnection *connection, const char *sender, const char *object_path,
    const char *interface_name, const char *method_name, GVariant *parameters,
    GDBusMethodInvocation *invocation, gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  CompatConnection *entry = user_data;
  if (g_str_equal(method_name, "GetSettings")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@a{sa{sv}})", public_settings(entry->settings)));
  } else if (g_str_equal(method_name, "GetSecrets")) {
    const char *setting_name;
    g_variant_get(parameters, "(&s)", &setting_name);
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@a{sa{sv}})", secret_settings(entry->settings, setting_name)));
  } else if (g_str_equal(method_name, "Delete")) {
    for (guint i = active_connections ? active_connections->len : 0; i > 0; i--) {
      CompatActiveConnection *active = g_ptr_array_index(active_connections, i - 1);
      if (g_str_equal(active->connection_path, entry->path)) {
        GError *error = NULL;
        if (!deactivate_active(active, &error)) {
          g_dbus_method_invocation_return_gerror(invocation, error);
          g_clear_error(&error);
          return;
        }
        g_ptr_array_remove_index(active_connections, i - 1);
      }
    }
    emit_connection_removed(entry->path);
    emit_interfaces_removed(entry->path);
    g_dbus_method_invocation_return_value(invocation, NULL);
    g_ptr_array_remove(connections, entry);
  } else {
    g_dbus_method_invocation_return_dbus_error(
        invocation, "org.freedesktop.NetworkManager.Error.NotSupported", method_name);
  }
}
static GVariant *connection_get_property(
    GDBusConnection *connection, const char *sender, const char *object_path,
    const char *interface_name, const char *property_name, GError **error,
    gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)user_data;
  if (g_str_equal(property_name, "Unsaved"))
    return g_variant_new_boolean(TRUE);
  if (g_str_equal(property_name, "Filename"))
    return g_variant_new_string("");
  if (g_str_equal(property_name, "Flags"))
    return g_variant_new_uint32(1u);
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
              "unsupported NetworkManager connection property %s", property_name);
  return NULL;
}

static const GDBusInterfaceVTable connection_vtable = {
    .method_call = connection_method_call,
    .get_property = connection_get_property,
};

static void add_unsaved_connection(GVariant *parameters,
                                   GDBusMethodInvocation *invocation) {
  GVariant *settings;
  g_variant_get(parameters, "(@a{sa{sv}})", &settings);
  CompatConnection *entry = g_new0(CompatConnection, 1);
  entry->path = g_strdup_printf("%s/%u", SETTINGS_PATH, ++next_connection_id);
  entry->settings = settings;
  GDBusInterfaceInfo *info = g_dbus_node_info_lookup_interface(
      service_node, "org.freedesktop.NetworkManager.Settings.Connection");
  GError *error = NULL;
  entry->registration_id = g_dbus_connection_register_object(
      service_connection, entry->path, info, &connection_vtable, entry, NULL, &error);
  if (!entry->registration_id) {
    g_dbus_method_invocation_return_gerror(invocation, error);
    g_clear_error(&error);
    connection_free(entry);
    return;
  }
  g_ptr_array_add(connections, entry);
  emit_interfaces_added(entry->path);
  emit_connection_added(entry->path);
  g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", entry->path));
}

static GVariant *manager_properties(void) {
  GVariantBuilder properties;
  g_variant_builder_init(&properties, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(&properties, "{sv}", "Version",
                        g_variant_new_string("1.40.18-freebsd-compat"));
  g_variant_builder_add(&properties, "{sv}", "VersionInfo", empty_array("au"));
  g_variant_builder_add(&properties, "{sv}", "State", g_variant_new_uint32(70u));
  g_variant_builder_add(&properties, "{sv}", "Startup", g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "NetworkingEnabled",
                        g_variant_new_boolean(TRUE));
  g_variant_builder_add(&properties, "{sv}", "WirelessEnabled",
                        g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "WirelessHardwareEnabled",
                        g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "WwanEnabled",
                        g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "WwanHardwareEnabled",
                        g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "Connectivity",
                        g_variant_new_uint32(4u));
  g_variant_builder_add(&properties, "{sv}", "ConnectivityCheckAvailable",
                        g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "ConnectivityCheckEnabled",
                        g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "ConnectivityCheckUri",
                        g_variant_new_string(""));
  g_variant_builder_add(&properties, "{sv}", "ActiveConnections", active_paths());
  g_variant_builder_add(&properties, "{sv}", "PrimaryConnection",
                        g_variant_new_object_path("/"));
  g_variant_builder_add(&properties, "{sv}", "PrimaryConnectionType",
                        g_variant_new_string(""));
  g_variant_builder_add(&properties, "{sv}", "ActivatingConnection",
                        g_variant_new_object_path("/"));
  g_variant_builder_add(&properties, "{sv}", "Metered", g_variant_new_uint32(0u));
  g_variant_builder_add(&properties, "{sv}", "Devices", device_paths());
  g_variant_builder_add(&properties, "{sv}", "AllDevices", device_paths());
  g_variant_builder_add(&properties, "{sv}", "Capabilities", empty_array("au"));
  return g_variant_builder_end(&properties);
}

static GVariant *settings_properties(void) {
  GVariantBuilder properties;
  g_variant_builder_init(&properties, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(&properties, "{sv}", "Connections", connection_paths());
  g_variant_builder_add(&properties, "{sv}", "Hostname", g_variant_new_string(""));
  g_variant_builder_add(&properties, "{sv}", "CanModify", g_variant_new_boolean(TRUE));
  g_variant_builder_add(&properties, "{sv}", "VersionId", g_variant_new_uint64(next_connection_id));
  return g_variant_builder_end(&properties);
}

static GVariant *managed_objects(void) {
  GVariantBuilder objects;
  GVariantBuilder interfaces;
  g_variant_builder_init(&objects, G_VARIANT_TYPE("a{oa{sa{sv}}}"));
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("a{sa{sv}}"));
  g_variant_builder_add(&interfaces, "{s@a{sv}}", NM_NAME, manager_properties());
  g_variant_builder_add(&objects, "{o@a{sa{sv}}}", NM_PATH,
                        g_variant_builder_end(&interfaces));
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("a{sa{sv}}"));
  g_variant_builder_add(&interfaces, "{s@a{sv}}", NM_NAME ".Settings",
                        settings_properties());
  g_variant_builder_add(&objects, "{o@a{sa{sv}}}", SETTINGS_PATH,
                        g_variant_builder_end(&interfaces));
  for (guint i = 0; connections && i < connections->len; i++) {
    CompatConnection *entry = g_ptr_array_index(connections, i);
    g_variant_builder_add(&objects, "{o@a{sa{sv}}}", entry->path,
                          connection_interfaces());
  }
  for (guint i = 0; active_connections && i < active_connections->len; i++) {
    CompatActiveConnection *entry = g_ptr_array_index(active_connections, i);
    if (!entry->active)
      continue;
    g_variant_builder_init(&interfaces, G_VARIANT_TYPE("a{sa{sv}}"));
    g_variant_builder_add(&interfaces, "{s@a{sv}}", NM_NAME ".Connection.Active",
                          active_properties(entry));
    g_variant_builder_add(&objects, "{o@a{sa{sv}}}", entry->path,
                          g_variant_builder_end(&interfaces));
    g_variant_builder_init(&interfaces, G_VARIANT_TYPE("a{sa{sv}}"));
    g_variant_builder_add(&interfaces, "{s@a{sv}}", NM_NAME ".Device",
                          device_properties(entry));
    g_variant_builder_add(&objects, "{o@a{sa{sv}}}", entry->device_path,
                          g_variant_builder_end(&interfaces));
  }
  return g_variant_builder_end(&objects);
}


static GVariant *setting_value(GVariant *settings, const char *section,
                               const char *key) {
  GVariant *values = g_variant_lookup_value(settings, section,
                                             G_VARIANT_TYPE("a{sv}"));
  if (!values)
    return NULL;
  GVariant *value = g_variant_lookup_value(values, key, NULL);
  g_variant_unref(values);
  return value;
}

static gchar *setting_string(GVariant *settings, const char *section,
                             const char *key) {
  GVariant *value = setting_value(settings, section, key);
  if (!value)
    return g_strdup("");
  if (!g_variant_is_of_type(value, G_VARIANT_TYPE_STRING)) {
    g_variant_unref(value);
    return g_strdup("");
  }
  gchar *result = g_variant_dup_string(value, NULL);
  g_variant_unref(value);
  return result;
}

static CompatConnection *find_connection(const char *path) {
  for (guint i = 0; connections && i < connections->len; i++) {
    CompatConnection *entry = g_ptr_array_index(connections, i);
    if (g_str_equal(entry->path, path))
      return entry;
  }
  return NULL;
}

static CompatActiveConnection *find_active(const char *path) {
  for (guint i = 0; active_connections && i < active_connections->len; i++) {
    CompatActiveConnection *entry = g_ptr_array_index(active_connections, i);
    if (g_str_equal(entry->path, path))
      return entry;
  }
  return NULL;
}

static GVariant *active_paths(void) {
  GVariantBuilder paths;
  g_variant_builder_init(&paths, G_VARIANT_TYPE("ao"));
  for (guint i = 0; active_connections && i < active_connections->len; i++) {
    CompatActiveConnection *entry = g_ptr_array_index(active_connections, i);
    if (entry->active)
      g_variant_builder_add(&paths, "o", entry->path);
  }
  return g_variant_builder_end(&paths);
}

static GVariant *device_paths(void) {
  GVariantBuilder paths;
  g_variant_builder_init(&paths, G_VARIANT_TYPE("ao"));
  for (guint i = 0; active_connections && i < active_connections->len; i++) {
    CompatActiveConnection *entry = g_ptr_array_index(active_connections, i);
    if (entry->active)
      g_variant_builder_add(&paths, "o", entry->device_path);
  }
  return g_variant_builder_end(&paths);
}

static void emit_manager_changed(void) {
  GVariantBuilder changes;
  g_variant_builder_init(&changes, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(&changes, "{sv}", "ActiveConnections", active_paths());
  g_variant_builder_add(&changes, "{sv}", "Devices", device_paths());
  g_variant_builder_add(&changes, "{sv}", "AllDevices", device_paths());
  g_dbus_connection_emit_signal(service_connection, NULL, NM_PATH,
      "org.freedesktop.DBus.Properties", "PropertiesChanged",
      g_variant_new("(s@a{sv}@as)", NM_NAME, g_variant_builder_end(&changes),
                    empty_array("as")), NULL);
}

static gboolean valid_helper_field(const char *value) {
  if (!value || !*value || strlen(value) > 63)
    return FALSE;
  for (const char *cursor = value; *cursor; cursor++)
    if (!g_ascii_isalnum(*cursor) && *cursor != '-' && *cursor != '_')
      return FALSE;
  return TRUE;
}

static gboolean run_network_helper(const char *action, const char *type,
                                   GVariant *settings, const char *token,
                                   gchar **result, GError **error) {
  const gchar *argv[5] = { network_helper ?: NETWORK_HELPER, action, NULL, NULL, NULL };
  if (type) argv[2] = type;
  if (token) argv[type ? 3 : 2] = token;
  GSubprocessLauncher *launcher = g_subprocess_launcher_new(
      G_SUBPROCESS_FLAGS_STDIN_PIPE | G_SUBPROCESS_FLAGS_STDOUT_PIPE |
      G_SUBPROCESS_FLAGS_STDERR_PIPE);
  GSubprocess *process = g_subprocess_launcher_spawnv(launcher, argv, error);
  g_object_unref(launcher);
  if (!process)
    return FALSE;
  GBytes *input = g_bytes_new(NULL, 0);
  if (settings) {
    GVariant *normal = g_variant_get_normal_form(settings);
    g_bytes_unref(input);
    input = g_variant_get_data_as_bytes(normal);
    g_variant_unref(normal);
  }
  GBytes *stdout_bytes = NULL;
  GBytes *stderr_bytes = NULL;
  gboolean ok = g_subprocess_communicate(process, input, NULL, &stdout_bytes,
                                          &stderr_bytes, error);
  if (input)
    g_bytes_unref(input);
  if (!ok) {
    g_clear_pointer(&stdout_bytes, g_bytes_unref);
    g_clear_pointer(&stderr_bytes, g_bytes_unref);
    g_object_unref(process);
    return FALSE;
  }
  if (!g_subprocess_get_successful(process)) {
    gsize stderr_size = 0;
    const gchar *stderr_data = g_bytes_get_data(stderr_bytes, &stderr_size);
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                "network helper rejected %s: %.*s", action, (int)stderr_size,
                stderr_data ?: "");
    g_bytes_unref(stdout_bytes);
    g_bytes_unref(stderr_bytes);
    g_object_unref(process);
    return FALSE;
  }
  if (result) {
    gsize stdout_size = 0;
    const gchar *stdout_data = g_bytes_get_data(stdout_bytes, &stdout_size);
    *result = g_strndup(stdout_data, stdout_size);
    g_strstrip(*result);
  }
  g_bytes_unref(stdout_bytes);
  g_bytes_unref(stderr_bytes);
  g_object_unref(process);
  return TRUE;
}

static GVariant *active_properties(CompatActiveConnection *entry) {
  CompatConnection *connection = find_connection(entry->connection_path);
  gchar *id = connection ? setting_string(connection->settings, "connection", "id") : g_strdup("");
  gchar *uuid = connection ? setting_string(connection->settings, "connection", "uuid") : g_strdup("");
  GVariantBuilder properties;
  GVariantBuilder devices;
  g_variant_builder_init(&properties, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_init(&devices, G_VARIANT_TYPE("ao"));
  g_variant_builder_add(&devices, "o", entry->device_path);
  g_variant_builder_add(&properties, "{sv}", "Connection", g_variant_new_object_path(entry->connection_path));
  g_variant_builder_add(&properties, "{sv}", "SpecificObject", g_variant_new_object_path("/"));
  g_variant_builder_add(&properties, "{sv}", "Id", g_variant_new_string(id));
  g_variant_builder_add(&properties, "{sv}", "Uuid", g_variant_new_string(uuid));
  g_variant_builder_add(&properties, "{sv}", "Type", g_variant_new_string(entry->type));
  g_variant_builder_add(&properties, "{sv}", "Devices", g_variant_builder_end(&devices));
  g_variant_builder_add(&properties, "{sv}", "State", g_variant_new_uint32(entry->active ? 2u : 4u));
  g_variant_builder_add(&properties, "{sv}", "Default", g_variant_new_boolean(entry->active && entry->default_ipv4));
  g_variant_builder_add(&properties, "{sv}", "Default6", g_variant_new_boolean(entry->active && entry->default_ipv6));
  g_variant_builder_add(&properties, "{sv}", "Vpn", g_variant_new_boolean(FALSE));
  g_variant_builder_add(&properties, "{sv}", "Master", g_variant_new_object_path("/"));
  g_free(id);
  g_free(uuid);
  return g_variant_builder_end(&properties);
}

static GVariant *device_properties(CompatActiveConnection *entry) {
  GVariantBuilder properties;
  g_variant_builder_init(&properties, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(&properties, "{sv}", "Interface", g_variant_new_string(entry->interface_name));
  g_variant_builder_add(&properties, "{sv}", "IpInterface", g_variant_new_string(entry->interface_name));
  g_variant_builder_add(&properties, "{sv}", "DeviceType", g_variant_new_uint32(g_str_equal(entry->type, "wireguard") ? 29u : 0u));
  g_variant_builder_add(&properties, "{sv}", "State", g_variant_new_uint32(entry->active ? 100u : 30u));
  g_variant_builder_add(&properties, "{sv}", "ActiveConnection", g_variant_new_object_path(entry->active ? entry->path : "/"));
  return g_variant_builder_end(&properties);
}

static void emit_active_added(CompatActiveConnection *entry) {
  GVariantBuilder interfaces;
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("a{sa{sv}}"));
  g_variant_builder_add(&interfaces, "{s@a{sv}}", NM_NAME ".Connection.Active", active_properties(entry));
  g_dbus_connection_emit_signal(service_connection, NULL, OBJECT_MANAGER_PATH,
      "org.freedesktop.DBus.ObjectManager", "InterfacesAdded",
      g_variant_new("(o@a{sa{sv}})", entry->path, g_variant_builder_end(&interfaces)), NULL);
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("a{sa{sv}}"));
  g_variant_builder_add(&interfaces, "{s@a{sv}}", NM_NAME ".Device", device_properties(entry));
  g_dbus_connection_emit_signal(service_connection, NULL, OBJECT_MANAGER_PATH,
      "org.freedesktop.DBus.ObjectManager", "InterfacesAdded",
      g_variant_new("(o@a{sa{sv}})", entry->device_path, g_variant_builder_end(&interfaces)), NULL);
  emit_manager_changed();
}

static void emit_active_removed(CompatActiveConnection *entry) {
  GVariantBuilder interfaces;
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("as"));
  g_variant_builder_add(&interfaces, "s", NM_NAME ".Connection.Active");
  g_dbus_connection_emit_signal(service_connection, NULL, OBJECT_MANAGER_PATH,
      "org.freedesktop.DBus.ObjectManager", "InterfacesRemoved",
      g_variant_new("(o@as)", entry->path, g_variant_builder_end(&interfaces)), NULL);
  g_variant_builder_init(&interfaces, G_VARIANT_TYPE("as"));
  g_variant_builder_add(&interfaces, "s", NM_NAME ".Device");
  g_dbus_connection_emit_signal(service_connection, NULL, OBJECT_MANAGER_PATH,
      "org.freedesktop.DBus.ObjectManager", "InterfacesRemoved",
      g_variant_new("(o@as)", entry->device_path, g_variant_builder_end(&interfaces)), NULL);
  emit_manager_changed();
}

static GVariant *active_get_property(GDBusConnection *connection, const char *sender,
    const char *object_path, const char *interface_name, const char *property_name,
    GError **error, gpointer user_data) {
  (void)connection; (void)sender; (void)object_path; (void)interface_name;
  CompatActiveConnection *entry = user_data;
  GVariant *properties = active_properties(entry);
  GVariant *value = g_variant_lookup_value(properties, property_name, NULL);
  g_variant_unref(properties);
  if (value)
    return value;
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED, "unsupported active connection property %s", property_name);
  return NULL;
}

static GVariant *device_get_property(GDBusConnection *connection, const char *sender,
    const char *object_path, const char *interface_name, const char *property_name,
    GError **error, gpointer user_data) {
  (void)connection; (void)sender; (void)object_path; (void)interface_name;
  CompatActiveConnection *entry = user_data;
  GVariant *properties = device_properties(entry);
  GVariant *value = g_variant_lookup_value(properties, property_name, NULL);
  g_variant_unref(properties);
  if (value)
    return value;
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED, "unsupported device property %s", property_name);
  return NULL;
}

static const GDBusInterfaceVTable active_vtable = { .get_property = active_get_property };
static const GDBusInterfaceVTable device_vtable = { .get_property = device_get_property };

static gboolean deactivate_active(CompatActiveConnection *entry, GError **error) {
  if (!entry->active)
    return TRUE;
  if (!run_network_helper("deactivate", NULL, NULL, entry->token, NULL, error))
    return FALSE;
  entry->active = FALSE;
  emit_active_removed(entry);
  return TRUE;
}

static void active_connection_free(gpointer data) {
  CompatActiveConnection *entry = data;
  GError *error = NULL;
  if (!deactivate_active(entry, &error)) {
    trace_log("network-manager-compat: host cleanup for %s failed: %s\\n", entry->path,
              error->message);
    g_clear_error(&error);
  }
  if (service_connection && entry->registration_id)
    g_dbus_connection_unregister_object(service_connection, entry->registration_id);
  if (service_connection && entry->device_registration_id)
    g_dbus_connection_unregister_object(service_connection, entry->device_registration_id);
  g_free(entry->path); g_free(entry->connection_path); g_free(entry->device_path);
  g_free(entry->interface_name); g_free(entry->token); g_free(entry->type); g_free(entry);
}

static gboolean activate_connection(CompatConnection *connection,
                                    gchar **active_path, GError **error) {
  gchar *type = setting_string(connection->settings, "connection", "type");
  if (!g_str_equal(type, "wireguard") && !g_str_equal(type, "dummy")) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                "unsupported NetworkManager connection type %s", type);
    g_free(type);
    return FALSE;
  }
  gchar *response = NULL;
  if (!run_network_helper("activate", type, connection->settings, NULL, &response, error)) {
    g_free(type);
    return FALSE;
  }
  gchar **fields = g_strsplit(response, " ", 5);
  if (!fields[0] || !fields[1] || !fields[2] || !fields[3] || fields[4] || !valid_helper_field(fields[0]) ||
      !valid_helper_field(fields[1]) || (strcmp(fields[2], "0") != 0 && strcmp(fields[2], "1") != 0) ||
      (strcmp(fields[3], "0") != 0 && strcmp(fields[3], "1") != 0)) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "network helper returned invalid activation handle");
    g_strfreev(fields); g_free(response); g_free(type); return FALSE;
  }
  CompatActiveConnection *entry = g_new0(CompatActiveConnection, 1);
  entry->path = g_strdup_printf("%s/%u", ACTIVE_PATH, ++next_active_connection_id);
  entry->connection_path = g_strdup(connection->path);
  entry->device_path = g_strdup_printf("%s/%u", DEVICE_PATH, next_active_connection_id);
  entry->token = g_strdup(fields[0]); entry->interface_name = g_strdup(fields[1]);
  entry->type = type; entry->active = TRUE;
  entry->default_ipv4 = g_str_equal(fields[2], "1");
  entry->default_ipv6 = g_str_equal(fields[3], "1");
  GDBusInterfaceInfo *active_info = g_dbus_node_info_lookup_interface(service_node, NM_NAME ".Connection.Active");
  GDBusInterfaceInfo *device_info = g_dbus_node_info_lookup_interface(service_node, NM_NAME ".Device");
  entry->registration_id = g_dbus_connection_register_object(service_connection, entry->path, active_info, &active_vtable, entry, NULL, error);
  if (!entry->registration_id) { active_connection_free(entry); g_strfreev(fields); g_free(response); return FALSE; }
  entry->device_registration_id = g_dbus_connection_register_object(service_connection, entry->device_path, device_info, &device_vtable, entry, NULL, error);
  if (!entry->device_registration_id) { active_connection_free(entry); g_strfreev(fields); g_free(response); return FALSE; }
  g_ptr_array_add(active_connections, entry);
  emit_active_added(entry);
  *active_path = g_strdup(entry->path);
  g_strfreev(fields); g_free(response);
  return TRUE;
}
static void not_supported(GDBusMethodInvocation *invocation,
                          const char *method_name) {
  g_dbus_method_invocation_return_dbus_error(
      invocation, "org.freedesktop.NetworkManager.Error.NotSupported",
      method_name);
}

static void method_call(GDBusConnection *connection, const char *sender,
                        const char *object_path, const char *interface_name,
                        const char *method_name, GVariant *parameters,
                        GDBusMethodInvocation *invocation, gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)user_data;
  if (g_str_equal(interface_name, "org.freedesktop.DBus.ObjectManager") &&
      g_str_equal(method_name, "GetManagedObjects")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@a{oa{sa{sv}}})", managed_objects()));
  } else if (g_str_equal(interface_name, NM_NAME ".Settings") &&
             g_str_equal(method_name, "AddConnectionUnsaved")) {
    add_unsaved_connection(parameters, invocation);
  } else if (g_str_equal(interface_name, NM_NAME) &&
             g_str_equal(method_name, "ActivateConnection")) {
    const char *connection_path;
    const char *device_path;
    const char *specific_object;
    g_variant_get(parameters, "(&o&o&o)", &connection_path, &device_path, &specific_object);
    if (!g_str_equal(device_path, "/") || !g_str_equal(specific_object, "/")) {
      g_dbus_method_invocation_return_dbus_error(invocation,
          "org.freedesktop.NetworkManager.Error.InvalidProperty",
          "only automatic device activation is supported");
      return;
    }
    CompatConnection *profile = find_connection(connection_path);
    if (!profile) {
      g_dbus_method_invocation_return_dbus_error(invocation,
          "org.freedesktop.NetworkManager.Error.UnknownConnection", connection_path);
      return;
    }
    gchar *active_path = NULL;
    GError *error = NULL;
    if (!activate_connection(profile, &active_path, &error)) {
      g_dbus_method_invocation_return_dbus_error(invocation,
          "org.freedesktop.NetworkManager.Error.Failed", error->message);
      g_clear_error(&error);
      return;
    }
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", active_path));
    g_free(active_path);
  } else if (g_str_equal(interface_name, NM_NAME) &&
             g_str_equal(method_name, "DeactivateConnection")) {
    const char *active_path;
    g_variant_get(parameters, "(&o)", &active_path);
    CompatActiveConnection *active = find_active(active_path);
    if (!active) {
      g_dbus_method_invocation_return_dbus_error(invocation,
          "org.freedesktop.NetworkManager.Error.UnknownConnection", active_path);
      return;
    }
    GError *error = NULL;
    if (!deactivate_active(active, &error)) {
      g_dbus_method_invocation_return_dbus_error(invocation,
          "org.freedesktop.NetworkManager.Error.Failed", error->message);
      g_clear_error(&error);
      return;
    }
    g_dbus_method_invocation_return_value(invocation, NULL);
    g_ptr_array_remove(active_connections, active);
  } else if (g_str_equal(method_name, "ListConnections")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@ao)", connection_paths()));
  } else if (g_str_equal(method_name, "GetDevices") ||
             g_str_equal(method_name, "GetAllDevices")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@ao)", device_paths()));
  } else if (g_str_equal(method_name, "CheckConnectivity")) {
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(u)", 4u));
  } else if (g_str_equal(method_name, "GetPermissions")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@a{ss})", empty_array("a{ss}")));
  } else if (g_str_has_prefix(method_name, "Register") ||
             g_str_equal(method_name, "Unregister")) {
    g_dbus_method_invocation_return_value(invocation, NULL);
  } else if (g_str_equal(method_name, "Enable") &&
             g_str_equal(interface_name,
                         "org.freedesktop.NetworkManager.AgentManager")) {
    g_dbus_method_invocation_return_value(invocation, NULL);
  } else {
    not_supported(invocation, method_name);
  }
}

static GVariant *get_property(GDBusConnection *connection, const char *sender,
                              const char *object_path,
                              const char *interface_name,
                              const char *property_name, GError **error,
                              gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)user_data;

  if (g_str_equal(property_name, "Version"))
    return g_variant_new_string("1.40.18-freebsd-compat");
  if (g_str_equal(property_name, "VersionInfo") ||
      g_str_equal(property_name, "Capabilities"))
    return empty_array("au");
  if (g_str_equal(property_name, "State"))
    return g_variant_new_uint32(70u);
  if (g_str_equal(property_name, "Connectivity"))
    return g_variant_new_uint32(4u);
  if (g_str_equal(property_name, "Metered"))
    return g_variant_new_uint32(0u);
  if (g_str_equal(property_name, "NetworkingEnabled"))
    return g_variant_new_boolean(TRUE);
  if (g_str_equal(property_name, "Startup") ||
      g_str_equal(property_name, "WirelessEnabled") ||
      g_str_equal(property_name, "WirelessHardwareEnabled") ||
      g_str_equal(property_name, "WwanEnabled") ||
      g_str_equal(property_name, "WwanHardwareEnabled") ||
      g_str_equal(property_name, "ConnectivityCheckAvailable") ||
      g_str_equal(property_name, "ConnectivityCheckEnabled") ||
      g_str_equal(property_name, "ConnectivityCheckEnabled"))
    return g_variant_new_boolean(FALSE);
  if (g_str_equal(property_name, "CanModify"))
    return g_variant_new_boolean(TRUE);
  if (g_str_equal(property_name, "ConnectivityCheckUri") ||
      g_str_equal(property_name, "PrimaryConnectionType") ||
      g_str_equal(property_name, "Hostname"))
    return g_variant_new_string("");
  if (g_str_equal(property_name, "PrimaryConnection") ||
      g_str_equal(property_name, "ActivatingConnection"))
    return g_variant_new_object_path("/");
  if (g_str_equal(property_name, "Connections"))
    return connection_paths();
  if (g_str_equal(property_name, "VersionId"))
    return g_variant_new_uint64(next_connection_id);
  if (g_str_equal(property_name, "ActiveConnections"))
    return active_paths();
  if (g_str_equal(property_name, "Devices") ||
      g_str_equal(property_name, "AllDevices"))
    return device_paths();

  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
              "unsupported NetworkManager property %s", property_name);
  return NULL;
}

static gboolean set_property(GDBusConnection *connection, const char *sender,
                             const char *object_path,
                             const char *interface_name,
                             const char *property_name, GVariant *value,
                             GError **error, gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)value;
  (void)user_data;
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
              "unsupported NetworkManager property %s", property_name);
  return FALSE;
}

static const char *message_type_name(GDBusMessageType type) {
  switch (type) {
  case G_DBUS_MESSAGE_TYPE_METHOD_CALL:
    return "call";
  case G_DBUS_MESSAGE_TYPE_METHOD_RETURN:
    return "return";
  case G_DBUS_MESSAGE_TYPE_ERROR:
    return "error";
  case G_DBUS_MESSAGE_TYPE_SIGNAL:
    return "signal";
  default:
    return "message";
  }
}

static gboolean is_network_manager_message(GDBusMessage *message) {
  const char *path = g_dbus_message_get_path(message);
  const char *destination = g_dbus_message_get_destination(message);
  const char *sender = g_dbus_message_get_sender(message);
  return (path && (g_str_equal(path, OBJECT_MANAGER_PATH) ||
                   g_str_has_prefix(path, NM_PATH))) ||
         (destination && g_str_equal(destination, NM_NAME)) ||
         (sender && g_str_equal(sender, NM_NAME));
}

static void trace_log(const char *format, ...) {
  va_list args;
  va_start(args, format);
  vfprintf(stderr, format, args);
  va_end(args);
  if (!trace_output)
    return;
  va_start(args, format);
  vfprintf(trace_output, format, args);
  va_end(args);
  fflush(trace_output);
}

static gboolean is_safe_trace_body(const char *signature) {
  return g_str_equal(signature, "(a{ss})") || g_str_equal(signature, "(u)") ||
         g_str_equal(signature, "(b)") || g_str_equal(signature, "(s)") || g_str_equal(signature, "(ss)") ||
         g_str_equal(signature, "(o)") || g_str_equal(signature, "(ooo)") ||
         g_str_equal(signature, "(ao)");
}

static char *trace_profile_sections(GVariant *body) {
  if (!body || !g_variant_is_of_type(body, G_VARIANT_TYPE("(a{sa{sv}})"))) return NULL;
  GVariant *settings = g_variant_get_child_value(body, 0);
  GVariantIter sections_iter; const char *section; GVariant *values;
  GString *sections = g_string_new(NULL);
  g_variant_iter_init(&sections_iter, settings);
  while (g_variant_iter_next(&sections_iter, "{&s@a{sv}}", &section, &values)) {
    if (sections->len) g_string_append_c(sections, ","[0]);
    g_string_append_printf(sections, "%s[", section);
    GVariantIter keys_iter; const char *key; GVariant *field; gboolean first = TRUE;
    g_variant_iter_init(&keys_iter, values);
    while (g_variant_iter_next(&keys_iter, "{&s@v}", &key, &field)) {
      if (!first) g_string_append_c(sections, "+"[0]);
      g_string_append(sections, key); first = FALSE; g_variant_unref(field);
    }
    g_string_append_c(sections, "]"[0]); g_variant_unref(values);
  }
  g_variant_unref(settings); return g_string_free(sections, FALSE);
}
static GDBusMessage *trace_message(GDBusConnection *connection,
                                   GDBusMessage *message, gboolean incoming,
                                   gpointer user_data) {
  (void)connection;
  (void)user_data;
  GDBusMessageType type = g_dbus_message_get_message_type(message);
  if (!is_network_manager_message(message) &&
      (incoming || type == G_DBUS_MESSAGE_TYPE_METHOD_CALL))
    return message;

  const char *path = g_dbus_message_get_path(message);
  const char *interface_name = g_dbus_message_get_interface(message);
  const char *member = g_dbus_message_get_member(message);
  GVariant *body = g_dbus_message_get_body(message);
  const char *signature = body ? g_variant_get_type_string(body) : "()";
  const char *error_name = g_dbus_message_get_error_name(message);
  char *body_text =
      body && is_safe_trace_body(signature) ? g_variant_print(body, FALSE) : NULL;
  char *profile_sections = trace_profile_sections(body);
  trace_log(
      "network-manager-compat: %s type=%s path=%s interface=%s member=%s "
      "signature=%s%s%s%s%s%s%s\n",
      incoming ? "in" : "out", message_type_name(type), path ?: "-",
      interface_name ?: "-", member ?: "-", signature,
      error_name ? " error=" : "", error_name ?: "",
      body_text ? " body=" : "", body_text ?: "",
      profile_sections ? " sections=" : "", profile_sections ?: "");
  g_free(profile_sections);
  g_free(body_text);
  return message;
}

static const GDBusInterfaceVTable vtable = {
    .method_call = method_call,
    .get_property = get_property,
    .set_property = set_property,
};

static gboolean stop_loop(gpointer data) {
  g_main_loop_quit(data);
  return G_SOURCE_REMOVE;
}

static gboolean register_interface(GDBusConnection *connection,
                                   GDBusNodeInfo *node, const char *path,
                                   const char *interface_name, GError **error) {
  GDBusInterfaceInfo *info =
      g_dbus_node_info_lookup_interface(node, interface_name);
  return g_dbus_connection_register_object(connection, path, info, &vtable,
                                           NULL, NULL, error) != 0;
}

int main(int argc, char **argv) {
  if (argc < 3 || !g_str_equal(argv[1], "--address")) {
    g_printerr("usage: %s --address ADDRESS [--trace-file PATH] [--network-helper PATH]\n", argv[0]);
    return 64;
  }
  for (int index = 3; index < argc; index += 2) {
    if (index + 1 >= argc) {
      g_printerr("network-manager-compat: option %s needs a value\n", argv[index]);
      return 64;
    }
    if (g_str_equal(argv[index], "--trace-file")) {
      if (trace_output) { g_printerr("network-manager-compat: duplicate trace file\n"); return 64; }
      trace_output = fopen(argv[index + 1], "a");
    } else if (g_str_equal(argv[index], "--network-helper")) {
      if (network_helper) { g_printerr("network-manager-compat: duplicate network helper\n"); return 64; }
      network_helper = g_strdup(argv[index + 1]);
      continue;
    } else {
      g_printerr("network-manager-compat: unknown option %s\n", argv[index]);
      return 64;
    }
    if (!trace_output) {
      g_printerr("network-manager-compat: open trace failed: %s\n",
                 g_strerror(errno));
      return 1;
    }
  }

  GError *error = NULL;
  GDBusConnection *connection = g_dbus_connection_new_for_address_sync(
      argv[2], G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
                   G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
      NULL, NULL, &error);
  if (!connection) {
    g_printerr("network-manager-compat: connect failed: %s\n", error->message);
    g_error_free(error);
    return 1;
  }
  g_dbus_connection_add_filter(connection, trace_message, NULL, NULL);

  GDBusNodeInfo *node = g_dbus_node_info_new_for_xml(introspection_xml, &error);
  service_connection = connection;
  service_node = node;
  connections = g_ptr_array_new_with_free_func(connection_free);
  active_connections = g_ptr_array_new_with_free_func(active_connection_free);
  if (!node ||
      !register_interface(connection, node, OBJECT_MANAGER_PATH,
                          "org.freedesktop.DBus.ObjectManager", &error) ||
      !register_interface(connection, node, NM_PATH, NM_NAME, &error) ||
      !register_interface(connection, node, SETTINGS_PATH,
                          NM_NAME ".Settings", &error) ||
      !register_interface(connection, node, AGENT_PATH,
                          NM_NAME ".AgentManager", &error)) {
    g_printerr("network-manager-compat: registration failed: %s\n",
               error ? error->message : "unknown error");
  g_clear_pointer(&active_connections, g_ptr_array_unref);
    g_clear_pointer(&connections, g_ptr_array_unref);
    service_node = NULL;
    service_connection = NULL;
    g_clear_error(&error);
    if (node)
      g_dbus_node_info_unref(node);
    g_object_unref(connection);
    return 1;
  }

  GVariant *reply = g_dbus_connection_call_sync(
      connection, "org.freedesktop.DBus", "/org/freedesktop/DBus",
      "org.freedesktop.DBus", "RequestName", g_variant_new("(su)", NM_NAME, 0u),
      G_VARIANT_TYPE("(u)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
  guint32 request_result = 0;
  if (reply)
    g_variant_get(reply, "(u)", &request_result);
  if (!reply || request_result != 1u) {
  g_clear_pointer(&active_connections, g_ptr_array_unref);
    g_clear_pointer(&connections, g_ptr_array_unref);
    service_node = NULL;
    service_connection = NULL;
    g_printerr("network-manager-compat: RequestName failed: %s\n",
               error ? error->message : "name is unavailable");
    g_clear_error(&error);
    if (reply)
      g_variant_unref(reply);
    g_dbus_node_info_unref(node);
    g_object_unref(connection);
    return 1;
  }
  g_variant_unref(reply);

  GMainLoop *loop = g_main_loop_new(NULL, FALSE);
  g_unix_signal_add(SIGINT, stop_loop, loop);
  g_unix_signal_add(SIGTERM, stop_loop, loop);
  g_printerr("network-manager-compat: serving NetworkManager compatibility API\n");
  g_main_loop_run(loop);

  g_dbus_connection_flush_sync(connection, NULL, NULL);
  g_main_loop_unref(loop);
  g_clear_pointer(&active_connections, g_ptr_array_unref);
  g_clear_pointer(&connections, g_ptr_array_unref);
  service_node = NULL;
  service_connection = NULL;
  g_dbus_node_info_unref(node);
  if (trace_output)
    fclose(trace_output);
  g_free(network_helper);
  g_object_unref(connection);
  return 0;
}
