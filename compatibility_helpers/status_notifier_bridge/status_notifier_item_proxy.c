#include "status_notifier_item_proxy.h"
#include "dbusmenu_proxy.h"
const char *STATUS_ITEM_XML =
    "<node>"
    "  <interface name='org.kde.StatusNotifierItem'>"
    "    <property name='Category' type='s' access='read'/>"
    "    <property name='Id' type='s' access='read'/>"
    "    <property name='Title' type='s' access='read'/>"
    "    <property name='Status' type='s' access='read'/>"
    "    <property name='WindowId' type='u' access='read'/>"
    "    <property name='IconName' type='s' access='read'/>"
    "    <property name='IconPixmap' type='a(iiay)' access='read'/>"
    "    <property name='OverlayIconName' type='s' access='read'/>"
    "    <property name='OverlayIconPixmap' type='a(iiay)' access='read'/>"
    "    <property name='AttentionIconName' type='s' access='read'/>"
    "    <property name='AttentionIconPixmap' type='a(iiay)' access='read'/>"
    "    <property name='AttentionMovieName' type='s' access='read'/>"
    "    <property name='ToolTip' type='(sa(iiay)ss)' access='read'/>"
    "    <property name='ItemIsMenu' type='b' access='read'/>"
    "    <property name='Menu' type='o' access='read'/>"
    "    <method name='ContextMenu'>"
    "      <arg type='i' name='x' direction='in'/>"
    "      <arg type='i' name='y' direction='in'/>"
    "    </method>"
    "    <method name='Activate'>"
    "      <arg type='i' name='x' direction='in'/>"
    "      <arg type='i' name='y' direction='in'/>"
    "    </method>"
    "    <method name='SecondaryActivate'>"
    "      <arg type='i' name='x' direction='in'/>"
    "      <arg type='i' name='y' direction='in'/>"
    "    </method>"
    "    <method name='Scroll'>"
    "      <arg type='i' name='delta' direction='in'/>"
    "      <arg type='s' name='orientation' direction='in'/>"
    "    </method>"
    "    <signal name='NewTitle'/>"
    "    <signal name='NewIcon'/>"
    "    <signal name='NewAttentionIcon'/>"
    "    <signal name='NewOverlayIcon'/>"
    "    <signal name='NewToolTip'/>"
    "    <signal name='NewStatus'>"
    "      <arg type='s' name='status'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

void free_status_item(StatusItem *item) {
  if (item == NULL) {
    return;
  }
  if (item->local_signal_id != 0 && item->state->local_bus != NULL) {
    g_dbus_connection_signal_unsubscribe(item->state->local_bus,
                                         item->local_signal_id);
  }
  if (item->host_registration_id != 0 && item->state->host_bus != NULL) {
    g_dbus_connection_unregister_object(item->state->host_bus,
                                        item->host_registration_id);
  }
  if (item->menus != NULL) {
    g_ptr_array_free(item->menus, TRUE);
  }
  g_free(item->local_service);
  g_free(item->local_path);
  g_free(item->local_registration);
  g_free(item->host_path);
  g_free(item);
}

void status_notifier_forward_call(GObject *source_object, GAsyncResult *result,
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

GVariant *empty_icon_pixmap(void) {
  GVariantBuilder builder;
  g_variant_builder_init(&builder, G_VARIANT_TYPE("a(iiay)"));
  return g_variant_builder_end(&builder);
}

GVariant *empty_tooltip(void) {
  GVariantBuilder pixmap;
  g_variant_builder_init(&pixmap, G_VARIANT_TYPE("a(iiay)"));
  return g_variant_new("(s@a(iiay)ss)", "", g_variant_builder_end(&pixmap), "",
                       "");
}

GVariant *default_status_property(StatusItem *item, const char *property_name) {
  if (g_strcmp0(property_name, "Category") == 0) {
    return g_variant_new_string("ApplicationStatus");
  }
  if (g_strcmp0(property_name, "Id") == 0 ||
      g_strcmp0(property_name, "Title") == 0) {
    return g_variant_new_string(item->state->app_id);
  }
  if (g_strcmp0(property_name, "Status") == 0) {
    return g_variant_new_string("Active");
  }
  if (g_strcmp0(property_name, "WindowId") == 0) {
    return g_variant_new_uint32(0);
  }
  if (g_str_has_suffix(property_name, "IconName") ||
      g_strcmp0(property_name, "AttentionMovieName") == 0) {
    return g_variant_new_string("");
  }
  if (g_str_has_suffix(property_name, "IconPixmap")) {
    return empty_icon_pixmap();
  }
  if (g_strcmp0(property_name, "ToolTip") == 0) {
    return empty_tooltip();
  }
  if (g_strcmp0(property_name, "ItemIsMenu") == 0) {
    return g_variant_new_boolean(FALSE);
  }
  if (g_strcmp0(property_name, "Menu") == 0) {
    return g_variant_new_object_path("/");
  }
  return NULL;
}

void on_local_status_signal(GDBusConnection *connection,
                            const gchar *sender_name, const gchar *object_path,
                            const gchar *interface_name,
                            const gchar *signal_name, GVariant *parameters,
                            gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  StatusItem *item = user_data;
  if (!g_dbus_connection_emit_signal(
          item->state->host_bus, NULL, item->host_path, interface_name,
          signal_name, g_variant_ref(parameters), NULL)) {
    status_notifier_log("forward StatusNotifier signal %s failed", signal_name);
  }
}

GVariant *local_status_property(StatusItem *item, const char *property_name) {
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_sync(
      item->state->local_bus, item->local_service, item->local_path,
      "org.freedesktop.DBus.Properties", "Get",
      g_variant_new("(ss)", "org.kde.StatusNotifierItem", property_name),
      G_VARIANT_TYPE("(v)"), G_DBUS_CALL_FLAGS_NONE, 1000, NULL, &error);
  if (reply == NULL) {
    status_notifier_log("StatusNotifier property %s.%s unavailable: %s",
                        item->local_service, property_name, error->message);
    g_error_free(error);
    return default_status_property(item, property_name);
  }

  GVariant *boxed = g_variant_get_child_value(reply, 0);
  GVariant *value = g_variant_get_variant(boxed);
  g_variant_unref(boxed);
  g_variant_unref(reply);

  if (g_strcmp0(property_name, "Menu") == 0 &&
      g_variant_is_of_type(value, G_VARIANT_TYPE_OBJECT_PATH)) {
    MenuProxy *menu =
        ensure_menu_proxy(item, g_variant_get_string(value, NULL));
    if (menu != NULL) {
      g_variant_unref(value);
      return g_variant_new_object_path(menu->host_path);
    }
  }
  return value;
}

void handle_status_item_method(GDBusConnection *connection, const gchar *sender,
                               const gchar *object_path,
                               const gchar *interface_name,
                               const gchar *method_name, GVariant *parameters,
                               GDBusMethodInvocation *invocation,
                               gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  StatusItem *item = user_data;
  g_dbus_connection_call(
      item->state->local_bus, item->local_service, item->local_path,
      interface_name, method_name, parameters, NULL, G_DBUS_CALL_FLAGS_NONE, -1,
      NULL, status_notifier_forward_call, g_object_ref(invocation));
}

GVariant *handle_status_item_property(GDBusConnection *connection,
                                      const gchar *sender,
                                      const gchar *object_path,
                                      const gchar *interface_name,
                                      const gchar *property_name,
                                      GError **error, gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)error;
  StatusItem *item = user_data;
  return local_status_property(item, property_name);
}

const GDBusInterfaceVTable STATUS_ITEM_VTABLE = {
    .method_call = handle_status_item_method,
    .get_property = handle_status_item_property,
};
