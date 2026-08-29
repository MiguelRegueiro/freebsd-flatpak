#include <gio/gio.h>
#include <glib-unix.h>
#include <stdio.h>
#include <stdarg.h>
#include <string.h>

#define NM_NAME "org.freedesktop.NetworkManager"
#define NM_PATH "/org/freedesktop/NetworkManager"
#define SETTINGS_PATH "/org/freedesktop/NetworkManager/Settings"
#define AGENT_PATH "/org/freedesktop/NetworkManager/AgentManager"
#define OBJECT_MANAGER_PATH "/org/freedesktop"

static FILE *trace_output;
typedef struct {
  gchar *path;
  GVariant *settings;
  guint registration_id;
} CompatConnection;

static GDBusConnection *service_connection;
static GDBusNodeInfo *service_node;
static GPtrArray *connections;
static guint next_connection_id;

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
    " </interface>"
    " <interface name='org.freedesktop.NetworkManager.Settings.Connection'>"
    "  <method name='GetSettings'><arg type='a{sa{sv}}' direction='out'/></method>"
    "  <method name='Delete'/>"
    "  <property name='Unsaved' type='b' access='read'/>"
    "  <property name='Filename' type='s' access='read'/>"
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

static void connection_method_call(
    GDBusConnection *connection, const char *sender, const char *object_path,
    const char *interface_name, const char *method_name, GVariant *parameters,
    GDBusMethodInvocation *invocation, gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)parameters;
  CompatConnection *entry = user_data;
  if (g_str_equal(method_name, "GetSettings")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@a{sa{sv}})", g_variant_ref(entry->settings)));
  } else if (g_str_equal(method_name, "Delete")) {
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
  g_variant_builder_add(&properties, "{sv}", "ActiveConnections", empty_array("ao"));
  g_variant_builder_add(&properties, "{sv}", "PrimaryConnection",
                        g_variant_new_object_path("/"));
  g_variant_builder_add(&properties, "{sv}", "PrimaryConnectionType",
                        g_variant_new_string(""));
  g_variant_builder_add(&properties, "{sv}", "ActivatingConnection",
                        g_variant_new_object_path("/"));
  g_variant_builder_add(&properties, "{sv}", "Metered", g_variant_new_uint32(0u));
  g_variant_builder_add(&properties, "{sv}", "Devices", empty_array("ao"));
  g_variant_builder_add(&properties, "{sv}", "AllDevices", empty_array("ao"));
  g_variant_builder_add(&properties, "{sv}", "Capabilities", empty_array("au"));
  return g_variant_builder_end(&properties);
}

static GVariant *settings_properties(void) {
  GVariantBuilder properties;
  g_variant_builder_init(&properties, G_VARIANT_TYPE("a{sv}"));
  g_variant_builder_add(&properties, "{sv}", "Connections", connection_paths());
  g_variant_builder_add(&properties, "{sv}", "Hostname", g_variant_new_string(""));
  g_variant_builder_add(&properties, "{sv}", "CanModify", g_variant_new_boolean(FALSE));
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
  return g_variant_builder_end(&objects);
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
  } else if (g_str_equal(method_name, "ListConnections")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@ao)", connection_paths()));
  } else if (g_str_equal(method_name, "GetDevices") ||
             g_str_equal(method_name, "GetAllDevices")) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@ao)", empty_array("ao")));
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
      g_str_equal(property_name, "CanModify"))
    return g_variant_new_boolean(FALSE);
  if (g_str_equal(property_name, "ConnectivityCheckUri") ||
      g_str_equal(property_name, "PrimaryConnectionType") ||
      g_str_equal(property_name, "Hostname"))
    return g_variant_new_string("");
  if (g_str_equal(property_name, "PrimaryConnection") ||
      g_str_equal(property_name, "ActivatingConnection"))
    return g_variant_new_object_path("/");
  if (g_str_equal(property_name, "Connections"))
    return connection_paths();
  if (g_str_equal(property_name, "ActiveConnections") ||
      g_str_equal(property_name, "Devices") ||
      g_str_equal(property_name, "AllDevices"))
    return empty_array("ao");

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
         g_str_equal(signature, "(b)") || g_str_equal(signature, "(s)") ||
         g_str_equal(signature, "(o)") || g_str_equal(signature, "(ooo)") ||
         g_str_equal(signature, "(ao)");
}

static char *trace_profile_sections(GVariant *body) {
  if (!body || !g_variant_is_of_type(body, G_VARIANT_TYPE("(a{sa{sv}})")))
    return NULL;
  GVariant *settings = g_variant_get_child_value(body, 0);
  GVariantIter iter;
  const char *section;
  GVariant *values;
  GString *sections = g_string_new(NULL);
  g_variant_iter_init(&iter, settings);
  while (g_variant_iter_next(&iter, "{&s@a{sv}}", &section, &values)) {
    if (sections->len)
      g_string_append_c(sections, ',');
    g_string_append(sections, section);
    g_variant_unref(values);
  }
  g_variant_unref(settings);
  return g_string_free(sections, FALSE);
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
  if ((argc != 3 && argc != 5) || !g_str_equal(argv[1], "--address") ||
      (argc == 5 && !g_str_equal(argv[3], "--trace-file"))) {
    g_printerr("usage: %s --address ADDRESS [--trace-file PATH]\n", argv[0]);
    return 64;
  }
  if (argc == 5) {
    trace_output = fopen(argv[4], "a");
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
  g_printerr("network-manager-compat: serving read-only NetworkManager API\n");
  g_main_loop_run(loop);

  g_dbus_connection_flush_sync(connection, NULL, NULL);
  g_main_loop_unref(loop);
  g_clear_pointer(&connections, g_ptr_array_unref);
  service_node = NULL;
  service_connection = NULL;
  g_dbus_node_info_unref(node);
  if (trace_output)
    fclose(trace_output);
  g_object_unref(connection);
  return 0;
}
