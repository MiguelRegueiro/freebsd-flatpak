#include <gio/gio.h>
#include <glib-unix.h>
#include <stdio.h>
#include <string.h>

#define NM_NAME "org.freedesktop.NetworkManager"
#define NM_PATH "/org/freedesktop/NetworkManager"
#define SETTINGS_PATH "/org/freedesktop/NetworkManager/Settings"
#define AGENT_PATH "/org/freedesktop/NetworkManager/AgentManager"

static const char *introspection_xml =
    "<node>"
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
  (void)parameters;
  (void)user_data;
  if (g_str_equal(method_name, "GetDevices") ||
      g_str_equal(method_name, "GetAllDevices") ||
      g_str_equal(method_name, "ListConnections")) {
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
  if (g_str_equal(property_name, "ActiveConnections") ||
      g_str_equal(property_name, "Devices") ||
      g_str_equal(property_name, "AllDevices") ||
      g_str_equal(property_name, "Connections"))
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

static GDBusMessage *trace_message(GDBusConnection *connection,
                                   GDBusMessage *message, gboolean incoming,
                                   gpointer user_data) {
  (void)connection;
  (void)user_data;
  if (!incoming ||
      g_dbus_message_get_message_type(message) !=
          G_DBUS_MESSAGE_TYPE_METHOD_CALL)
    return message;

  const char *path = g_dbus_message_get_path(message);
  const char *interface_name = g_dbus_message_get_interface(message);
  const char *member = g_dbus_message_get_member(message);
  if (path && g_str_has_prefix(path, NM_PATH) && interface_name && member)
    g_printerr("network-manager-compat: call %s %s.%s\n", path,
               interface_name, member);
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
  if (argc != 3 || !g_str_equal(argv[1], "--address")) {
    g_printerr("usage: %s --address ADDRESS\n", argv[0]);
    return 64;
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
  if (!node ||
      !register_interface(connection, node, NM_PATH, NM_NAME, &error) ||
      !register_interface(connection, node, SETTINGS_PATH,
                          NM_NAME ".Settings", &error) ||
      !register_interface(connection, node, AGENT_PATH,
                          NM_NAME ".AgentManager", &error)) {
    g_printerr("network-manager-compat: registration failed: %s\n",
               error ? error->message : "unknown error");
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
  g_dbus_node_info_unref(node);
  g_object_unref(connection);
  return 0;
}
