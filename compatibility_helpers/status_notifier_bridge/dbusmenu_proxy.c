#include "dbusmenu_proxy.h"
const char *DBUSMENU_XML =
    "<node>"
    "  <interface name='com.canonical.dbusmenu'>"
    "    <method name='GetLayout'>"
    "      <arg type='i' name='parentId' direction='in'/>"
    "      <arg type='i' name='recursionDepth' direction='in'/>"
    "      <arg type='as' name='propertyNames' direction='in'/>"
    "      <arg type='u' name='revision' direction='out'/>"
    "      <arg type='(ia{sv}av)' name='layout' direction='out'/>"
    "    </method>"
    "    <method name='GetGroupProperties'>"
    "      <arg type='ai' name='ids' direction='in'/>"
    "      <arg type='as' name='propertyNames' direction='in'/>"
    "      <arg type='a(ia{sv})' name='properties' direction='out'/>"
    "    </method>"
    "    <method name='GetProperty'>"
    "      <arg type='i' name='id' direction='in'/>"
    "      <arg type='s' name='name' direction='in'/>"
    "      <arg type='v' name='value' direction='out'/>"
    "    </method>"
    "    <method name='Event'>"
    "      <arg type='i' name='id' direction='in'/>"
    "      <arg type='s' name='eventId' direction='in'/>"
    "      <arg type='v' name='data' direction='in'/>"
    "      <arg type='u' name='timestamp' direction='in'/>"
    "    </method>"
    "    <method name='EventGroup'>"
    "      <arg type='a(isvu)' name='events' direction='in'/>"
    "      <arg type='ai' name='idErrors' direction='out'/>"
    "    </method>"
    "    <method name='AboutToShow'>"
    "      <arg type='i' name='id' direction='in'/>"
    "      <arg type='b' name='needUpdate' direction='out'/>"
    "    </method>"
    "    <method name='AboutToShowGroup'>"
    "      <arg type='ai' name='ids' direction='in'/>"
    "      <arg type='ai' name='updatesNeeded' direction='out'/>"
    "      <arg type='ai' name='idErrors' direction='out'/>"
    "    </method>"
    "    <signal name='ItemsPropertiesUpdated'>"
    "      <arg type='a(ia{sv})' name='updatedProps'/>"
    "      <arg type='a(ias)' name='removedProps'/>"
    "    </signal>"
    "    <signal name='LayoutUpdated'>"
    "      <arg type='u' name='revision'/>"
    "      <arg type='i' name='parent'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

void free_menu_proxy(MenuProxy *menu) {
  if (menu == NULL) {
    return;
  }
  if (menu->local_signal_id != 0 && menu->item->state->local_bus != NULL) {
    g_dbus_connection_signal_unsubscribe(menu->item->state->local_bus,
                                         menu->local_signal_id);
  }
  if (menu->host_registration_id != 0 && menu->item->state->host_bus != NULL) {
    g_dbus_connection_unregister_object(menu->item->state->host_bus,
                                        menu->host_registration_id);
  }
  g_free(menu->local_path);
  g_free(menu->host_path);
  g_free(menu);
}

void on_local_menu_signal(GDBusConnection *connection, const gchar *sender_name,
                          const gchar *object_path, const gchar *interface_name,
                          const gchar *signal_name, GVariant *parameters,
                          gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  MenuProxy *menu = user_data;
  if (!g_dbus_connection_emit_signal(
          menu->item->state->host_bus, NULL, menu->host_path, interface_name,
          signal_name, g_variant_ref(parameters), NULL)) {
    status_notifier_log("forward DBusMenu signal %s failed", signal_name);
  }
}

void handle_menu_method(GDBusConnection *connection, const gchar *sender,
                        const gchar *object_path, const gchar *interface_name,
                        const gchar *method_name, GVariant *parameters,
                        GDBusMethodInvocation *invocation, gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  MenuProxy *menu = user_data;
  g_dbus_connection_call(
      menu->item->state->local_bus, menu->item->local_service, menu->local_path,
      interface_name, method_name, parameters, NULL, G_DBUS_CALL_FLAGS_NONE, -1,
      NULL, status_notifier_forward_call, g_object_ref(invocation));
}

static const GDBusInterfaceVTable MENU_VTABLE = {
    .method_call = handle_menu_method,
};

MenuProxy *ensure_menu_proxy(StatusItem *item, const char *menu_path) {
  if (menu_path == NULL || g_strcmp0(menu_path, "/") == 0 ||
      *menu_path == '\0') {
    return NULL;
  }
  for (guint i = 0; i < item->menus->len; i++) {
    MenuProxy *menu = g_ptr_array_index(item->menus, i);
    if (g_strcmp0(menu->local_path, menu_path) == 0) {
      return menu;
    }
  }

  GDBusInterfaceInfo *iface = g_dbus_node_info_lookup_interface(
      item->state->dbusmenu_node, "com.canonical.dbusmenu");
  MenuProxy *menu = g_new0(MenuProxy, 1);
  menu->item = item;
  menu->local_path = g_strdup(menu_path);
  menu->host_path =
      g_strdup_printf("%s/Menu%u", item->host_path, item->menus->len + 1);

  GError *error = NULL;
  menu->host_registration_id = g_dbus_connection_register_object(
      item->state->host_bus, menu->host_path, iface, &MENU_VTABLE, menu, NULL,
      &error);
  if (menu->host_registration_id == 0) {
    status_notifier_log("register host DBusMenu proxy %s failed: %s",
                        menu->host_path, error->message);
    g_error_free(error);
    free_menu_proxy(menu);
    return NULL;
  }
  menu->local_signal_id = g_dbus_connection_signal_subscribe(
      item->state->local_bus, item->local_service, "com.canonical.dbusmenu",
      NULL, menu->local_path, NULL, G_DBUS_SIGNAL_FLAGS_NONE,
      on_local_menu_signal, menu, NULL);

  g_ptr_array_add(item->menus, menu);
  status_notifier_log("bridged DBusMenu %s -> host %s", menu->local_path,
                      menu->host_path);
  return menu;
}
