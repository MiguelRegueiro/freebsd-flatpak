#include "status_notifier_watcher.h"
#include "dbusmenu_proxy.h"
#include "status_notifier_item_proxy.h"
void status_notifier_log(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  fputs("status notifier bridge: ", stderr);
  vfprintf(stderr, fmt, ap);
  fputc('\n', stderr);
  va_end(ap);
}
void status_notifier_diagnostic(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  fputs("status notifier bridge: ", stdout);
  vfprintf(stdout, fmt, ap);
  fputc('\n', stdout);
  va_end(ap);
}
const char *STATUS_WATCHER_XML =
    "<node>"
    "  <interface name='org.kde.StatusNotifierWatcher'>"
    "    <property name='RegisteredStatusNotifierItems' type='as' "
    "access='read'/>"
    "    <property name='IsStatusNotifierHostRegistered' type='b' "
    "access='read'/>"
    "    <property name='ProtocolVersion' type='i' access='read'/>"
    "    <method name='RegisterStatusNotifierItem'>"
    "      <arg type='s' name='service' direction='in'/>"
    "    </method>"
    "    <method name='RegisterStatusNotifierHost'>"
    "      <arg type='s' name='service' direction='in'/>"
    "    </method>"
    "    <signal name='StatusNotifierItemRegistered'>"
    "      <arg type='s' name='service'/>"
    "    </signal>"
    "    <signal name='StatusNotifierItemUnregistered'>"
    "      <arg type='s' name='service'/>"
    "    </signal>"
    "    <signal name='StatusNotifierHostRegistered'/>"
    "    <signal name='StatusNotifierHostUnregistered'/>"
    "  </interface>"
    "</node>";

char *status_registration_string(const char *service) {
  return g_strdup(service);
}

StatusItem *find_status_item(StatusNotifierBridge *state,
                             const char *local_service,
                             const char *local_path) {
  for (guint i = 0; i < state->status_items->len; i++) {
    StatusItem *item = g_ptr_array_index(state->status_items, i);
    if (g_strcmp0(item->local_service, local_service) == 0 &&
        g_strcmp0(item->local_path, local_path) == 0) {
      return item;
    }
  }
  return NULL;
}

bool register_host_status_item(StatusItem *item, GError **error) {
  GDBusInterfaceInfo *iface = g_dbus_node_info_lookup_interface(
      item->state->item_node, "org.kde.StatusNotifierItem");
  item->host_registration_id = g_dbus_connection_register_object(
      item->state->host_bus, item->host_path, iface, &STATUS_ITEM_VTABLE, item,
      NULL, error);
  if (item->host_registration_id == 0) {
    return false;
  }

  item->local_signal_id = g_dbus_connection_signal_subscribe(
      item->state->local_bus, item->local_service, "org.kde.StatusNotifierItem",
      NULL, item->local_path, NULL, G_DBUS_SIGNAL_FLAGS_NONE,
      on_local_status_signal, item, NULL);
  return true;
}

bool register_with_host_watcher(StatusItem *item, GError **error) {
  GVariant *reply = g_dbus_connection_call_sync(
      item->state->host_bus, "org.kde.StatusNotifierWatcher",
      "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
      "RegisterStatusNotifierItem", g_variant_new("(s)", item->host_path), NULL,
      G_DBUS_CALL_FLAGS_NONE, 2000, NULL, error);
  if (reply == NULL) {
    return false;
  }
  g_variant_unref(reply);
  return true;
}

void emit_local_status_item_registered(StatusItem *item) {
  g_dbus_connection_emit_signal(
      item->state->local_bus, NULL, "/StatusNotifierWatcher",
      "org.kde.StatusNotifierWatcher", "StatusNotifierItemRegistered",
      g_variant_new("(s)", item->local_registration), NULL);
}

void handle_register_status_item(StatusNotifierBridge *state,
                                 const char *sender, GVariant *parameters,
                                 GDBusMethodInvocation *invocation) {
  const char *service = NULL;
  g_variant_get(parameters, "(&s)", &service);
  const char *local_service = service;
  const char *local_path = "/StatusNotifierItem";
  if (g_str_has_prefix(service, "/")) {
    local_service = sender;
    local_path = service;
  }

  StatusItem *existing = find_status_item(state, local_service, local_path);
  if (existing != NULL) {
    emit_local_status_item_registered(existing);
    g_dbus_method_invocation_return_value(invocation, NULL);
    return;
  }

  StatusItem *item = g_new0(StatusItem, 1);
  item->state = state;
  item->local_service = g_strdup(local_service);
  item->local_path = g_strdup(local_path);
  item->local_registration = status_registration_string(service);
  item->host_path = g_strdup_printf(
      "/StatusNotifierItem/freebsd_flatpak_%" G_GUINT64_FORMAT,
      ++state->status_counter);
  item->menus = g_ptr_array_new_with_free_func((GDestroyNotify)free_menu_proxy);

  GError *error = NULL;
  if (!register_host_status_item(item, &error)) {
    g_dbus_method_invocation_take_error(invocation, error);
    free_status_item(item);
    return;
  }
  if (!register_with_host_watcher(item, &error)) {
    g_dbus_method_invocation_take_error(invocation, error);
    free_status_item(item);
    return;
  }

  g_ptr_array_add(state->status_items, item);
  emit_local_status_item_registered(item);
  g_dbus_method_invocation_return_value(invocation, NULL);
  status_notifier_diagnostic("bridged StatusNotifierItem %s%s -> host %s",
                             item->local_service, item->local_path,
                             item->host_path);
}

void handle_status_watcher_method(
    GDBusConnection *connection, const gchar *sender, const gchar *object_path,
    const gchar *interface_name, const gchar *method_name, GVariant *parameters,
    GDBusMethodInvocation *invocation, gpointer user_data) {
  (void)connection;
  (void)object_path;
  (void)interface_name;
  StatusNotifierBridge *state = user_data;
  if (g_strcmp0(method_name, "RegisterStatusNotifierItem") == 0) {
    handle_register_status_item(state, sender, parameters, invocation);
    return;
  }
  if (g_strcmp0(method_name, "RegisterStatusNotifierHost") == 0) {
    g_dbus_method_invocation_return_value(invocation, NULL);
    return;
  }
  g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                        G_IO_ERROR_NOT_SUPPORTED,
                                        "%s is not implemented", method_name);
}

GVariant *handle_status_watcher_property(GDBusConnection *connection,
                                         const gchar *sender,
                                         const gchar *object_path,
                                         const gchar *interface_name,
                                         const gchar *property_name,
                                         GError **error, gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  StatusNotifierBridge *state = user_data;
  if (g_strcmp0(property_name, "RegisteredStatusNotifierItems") == 0) {
    GVariantBuilder items;
    g_variant_builder_init(&items, G_VARIANT_TYPE("as"));
    for (guint i = 0; i < state->status_items->len; i++) {
      StatusItem *item = g_ptr_array_index(state->status_items, i);
      g_variant_builder_add(&items, "s", item->local_registration);
    }
    return g_variant_builder_end(&items);
  }
  if (g_strcmp0(property_name, "IsStatusNotifierHostRegistered") == 0) {
    return g_variant_new_boolean(TRUE);
  }
  if (g_strcmp0(property_name, "ProtocolVersion") == 0) {
    return g_variant_new_int32(0);
  }
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s",
              property_name);
  return NULL;
}

const GDBusInterfaceVTable STATUS_WATCHER_VTABLE = {
    .method_call = handle_status_watcher_method,
    .get_property = handle_status_watcher_property,
};
