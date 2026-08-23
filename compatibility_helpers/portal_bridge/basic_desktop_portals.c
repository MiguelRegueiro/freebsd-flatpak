#include "basic_desktop_portals.h"
#include "file_chooser_portal.h"
#include "portal_bridge_process.h"
#include "screencast_portal.h"
const char *DESKTOP_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.FileChooser'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='OpenFile'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SaveFile'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SaveFiles'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.Settings'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Read'>"
    "      <arg type='s' name='namespace' direction='in'/>"
    "      <arg type='s' name='key' direction='in'/>"
    "      <arg type='v' name='value' direction='out'/>"
    "    </method>"
    "    <method name='ReadAll'>"
    "      <arg type='as' name='namespaces' direction='in'/>"
    "      <arg type='a{sa{sv}}' name='values' direction='out'/>"
    "    </method>"
    "    <signal name='SettingChanged'>"
    "      <arg type='s' name='namespace'/>"
    "      <arg type='s' name='key'/>"
    "      <arg type='v' name='value'/>"
    "    </signal>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.ProxyResolver'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Lookup'>"
    "      <arg type='s' name='uri' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='as' name='proxies' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.Inhibit'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Inhibit'>"
    "      <arg type='s' name='window' direction='in'/>"
    "      <arg type='u' name='flags' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='CreateMonitor'>"
    "      <arg type='s' name='window' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='QueryEndResponse'>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "    </method>"
    "    <signal name='StateChanged'>"
    "      <arg type='o' name='session_handle'/>"
    "      <arg type='a{sv}' name='state'/>"
    "    </signal>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.ScreenCast'>"
    "    <property name='AvailableSourceTypes' type='u' access='read'/>"
    "    <property name='AvailableCursorModes' type='u' access='read'/>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='CreateSession'>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SelectSources'>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='Start'>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='OpenPipeWireRemote'>"
    "      <annotation name='org.gtk.GDBus.C.UnixFD' value='true'/>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='h' name='fd' direction='out'/>"
    "    </method>"
    "  </interface>"
    "</node>";

void on_forward_call(GObject *source_object, GAsyncResult *result,
                     gpointer user_data) {
  GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
  GDBusMethodInvocation *invocation = user_data;
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
  if (reply == NULL) {
    g_dbus_method_invocation_take_error(invocation, error);
  } else {
    g_dbus_method_invocation_return_value(invocation, reply);
  }
  g_object_unref(invocation);
}
char *fresh_request_path(BridgeState *state, const char *label) {
  return g_strdup_printf("/org/freedesktop/portal/desktop/request/"
                         "freebsd_flatpak_poc/%s_%" G_GUINT64_FORMAT,
                         label, ++state->request_store.request_counter);
}

void return_immediate_empty_request(BridgeState *state,
                                    GDBusMethodInvocation *invocation,
                                    const char *label) {
  char *path = fresh_request_path(state, label);
  g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", path));
  GVariantBuilder results;
  g_variant_builder_init(&results, G_VARIANT_TYPE("a{sv}"));
  g_dbus_connection_emit_signal(
      state->local_bus, NULL, path, "org.freedesktop.portal.Request",
      "Response",
      g_variant_new("(u@a{sv})", 2, g_variant_builder_end(&results)), NULL);
  g_free(path);
}

void forward_desktop_method(BridgeState *state, const char *interface_name,
                            const char *method_name, GVariant *parameters,
                            GDBusMethodInvocation *invocation) {
  g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                         "/org/freedesktop/portal/desktop", interface_name,
                         method_name, parameters, NULL, G_DBUS_CALL_FLAGS_NONE,
                         -1, NULL, on_forward_call, g_object_ref(invocation));
}

void return_settings_readall(GDBusMethodInvocation *invocation) {
  GVariantBuilder values;
  g_variant_builder_init(&values, G_VARIANT_TYPE("a{sa{sv}}"));
  g_dbus_method_invocation_return_value(invocation,
                                        g_variant_new("(a{sa{sv}})", &values));
}

void return_proxy_direct(GDBusMethodInvocation *invocation) {
  const char *direct[] = {"direct://", NULL};
  g_dbus_method_invocation_return_value(invocation,
                                        g_variant_new("(^as)", direct));
}

void handle_desktop_method(GDBusConnection *connection, const gchar *sender,
                           const gchar *object_path,
                           const gchar *interface_name,
                           const gchar *method_name, GVariant *parameters,
                           GDBusMethodInvocation *invocation,
                           gpointer user_data) {
  (void)connection;
  (void)object_path;

  BridgeState *state = user_data;
  if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") == 0 &&
      g_strcmp0(method_name, "CreateSession") == 0) {
    handle_screencast_create(state, sender, parameters, invocation);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") ==
                 0 &&
             (g_strcmp0(method_name, "SelectSources") == 0 ||
              g_strcmp0(method_name, "Start") == 0)) {
    handle_screencast_request(state, sender, method_name, parameters,
                              invocation);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") ==
                 0 &&
             g_strcmp0(method_name, "OpenPipeWireRemote") == 0) {
    handle_open_pipewire_remote(state, sender, parameters, invocation);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") ==
                 0 &&
             g_strcmp0(method_name, "OpenFile") == 0) {
    handle_filechooser_open(state, sender, parameters, invocation);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") ==
             0) {
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
        "%s is not implemented by this V1 bridge", method_name);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") ==
                 0 &&
             g_strcmp0(method_name, "ReadAll") == 0) {
    forward_desktop_method(state, interface_name, method_name, parameters,
                           invocation);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") ==
                 0 &&
             g_strcmp0(method_name, "Read") == 0) {
    forward_desktop_method(state, interface_name, method_name, parameters,
                           invocation);
  } else if (g_strcmp0(interface_name,
                       "org.freedesktop.portal.ProxyResolver") == 0 &&
             g_strcmp0(method_name, "Lookup") == 0) {
    return_proxy_direct(invocation);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0 &&
             (g_strcmp0(method_name, "CreateMonitor") == 0 ||
              g_strcmp0(method_name, "Inhibit") == 0)) {
    return_immediate_empty_request(state, invocation, method_name);
  } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0 &&
             g_strcmp0(method_name, "QueryEndResponse") == 0) {
    g_dbus_method_invocation_return_value(invocation, NULL);
  } else {
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
        "%s.%s is not implemented by this V1 bridge", interface_name,
        method_name);
  }
}

GVariant *handle_get_property(GDBusConnection *connection, const gchar *sender,
                              const gchar *object_path,
                              const gchar *interface_name,
                              const gchar *property_name, GError **error,
                              gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  BridgeState *state = user_data;

  if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") == 0) {
    if (g_strcmp0(property_name, "version") == 0) {
      return g_variant_new_uint32(state->screencast.version);
    }
    if (g_strcmp0(property_name, "AvailableSourceTypes") == 0) {
      return g_variant_new_uint32(state->screencast.source_types);
    }
    if (g_strcmp0(property_name, "AvailableCursorModes") == 0) {
      return g_variant_new_uint32(state->screencast.cursor_modes);
    }
  }

  if (g_strcmp0(property_name, "version") == 0) {
    if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") == 0) {
      return g_variant_new_uint32(4);
    }
    if (g_strcmp0(interface_name, "org.freedesktop.portal.Documents") == 0) {
      return g_variant_new_uint32(5);
    }
    if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") == 0) {
      return g_variant_new_uint32(2);
    }
    if (g_strcmp0(interface_name, "org.freedesktop.portal.ProxyResolver") ==
        0) {
      return g_variant_new_uint32(1);
    }
    if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0) {
      return g_variant_new_uint32(3);
    }
    if (g_strcmp0(interface_name, "org.freedesktop.portal.FileTransfer") == 0) {
      return g_variant_new_uint32(1);
    }
  }
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s.%s",
              interface_name, property_name);
  return NULL;
}

const GDBusInterfaceVTable DESKTOP_VTABLE = {
    .method_call = handle_desktop_method,
    .get_property = handle_get_property,
};
