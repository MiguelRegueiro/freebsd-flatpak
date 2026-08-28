#include "dbusmenu_proxy.h"
#include "status_notifier_item_proxy.h"
#include "status_notifier_watcher.h"

static gboolean handle_signal(gpointer user_data) {
  StatusNotifierBridge *state = user_data;
  g_main_loop_quit(state->loop);
  return G_SOURCE_REMOVE;
}

static bool register_interfaces(GDBusConnection *connection, const char *path,
                                GDBusNodeInfo *node,
                                const GDBusInterfaceVTable *vtable,
                                StatusNotifierBridge *state, GError **error) {
  for (guint i = 0; node->interfaces[i] != NULL; i++) {
    if (g_dbus_connection_register_object(connection, path, node->interfaces[i],
                                          vtable, state, NULL, error) == 0) {
      return false;
    }
  }
  return true;
}

static void on_bus_acquired(GDBusConnection *connection, const gchar *name,
                            gpointer user_data) {
  (void)name;
  StatusNotifierBridge *state = user_data;
  if (state->local_bus == NULL) {
    state->local_bus = g_object_ref(connection);
  }
  if (state->local_objects_registered) {
    return;
  }
  GError *error = NULL;
  if (!register_interfaces(connection, "/StatusNotifierWatcher",
                           state->watcher_node, &STATUS_WATCHER_VTABLE, state,
                           &error)) {
    status_notifier_log("register StatusNotifierWatcher failed: %s",
                        error->message);
    g_error_free(error);
    g_main_loop_quit(state->loop);
    return;
  }
  state->local_objects_registered = true;
}

static void on_name_acquired(GDBusConnection *connection, const gchar *name,
                             gpointer user_data) {
  (void)connection;
  (void)user_data;
  status_notifier_diagnostic("acquired %s", name);
}

static void on_name_lost(GDBusConnection *connection, const gchar *name,
                         gpointer user_data) {
  (void)connection;
  StatusNotifierBridge *state = user_data;
  status_notifier_log("lost %s", name);
  g_main_loop_quit(state->loop);
}

static const char *arg_value(int argc, char **argv, const char *name) {
  for (int i = 1; i + 1 < argc; i++) {
    if (strcmp(argv[i], name) == 0)
      return argv[i + 1];
  }
  return NULL;
}

static GDBusConnection *connect_to_bus_address(const char *address,
                                               GError **error) {
  return g_dbus_connection_new_for_address_sync(
      address,
      G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
          G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
      NULL, NULL, error);
}

int main(int argc, char **argv) {
  const char *app_id = arg_value(argc, argv, "--app-id");
  const char *shared_dir = arg_value(argc, argv, "--shared-dir");
  const char *app_root = arg_value(argc, argv, "--app-root");
  const char *runtime_root = arg_value(argc, argv, "--runtime-root");
  const char *host_address = getenv("HOST_DBUS_SESSION_BUS_ADDRESS");
  if (app_id == NULL || shared_dir == NULL || app_root == NULL ||
      runtime_root == NULL || host_address == NULL ||
      *host_address == '\0') {
    fprintf(stderr,
            "usage: %s --app-id APP_ID --shared-dir APP_PORTAL_DIR "
            "--app-root APP_FILES --runtime-root RUNTIME_FILES\n",
            argv[0]);
    fprintf(
        stderr,
        "HOST_DBUS_SESSION_BUS_ADDRESS must point at the host session bus\n");
    return 64;
  }
  GError *error = NULL;
  StatusNotifierBridge state = {
      .app_id = g_strdup(app_id),
      .app_root = g_strdup(app_root),
      .runtime_root = g_strdup(runtime_root),
      .loop = g_main_loop_new(NULL, FALSE),
      .host_bus = connect_to_bus_address(host_address, &error),
      .local_bus = NULL,
      .watcher_node = g_dbus_node_info_new_for_xml(STATUS_WATCHER_XML, &error),
      .item_node = g_dbus_node_info_new_for_xml(STATUS_ITEM_XML, &error),
      .dbusmenu_node = g_dbus_node_info_new_for_xml(DBUSMENU_XML, &error),
      .status_items =
          g_ptr_array_new_with_free_func((GDestroyNotify)free_status_item),
  };
  if (state.host_bus == NULL || state.watcher_node == NULL ||
      state.item_node == NULL || state.dbusmenu_node == NULL) {
    fprintf(stderr, "status notifier bridge setup failed: %s\n",
            error->message);
    g_error_free(error);
    return 1;
  }
  g_unix_signal_add(SIGINT, handle_signal, &state);
  g_unix_signal_add(SIGTERM, handle_signal, &state);
  guint owner_id = g_bus_own_name(
      G_BUS_TYPE_SESSION, "org.kde.StatusNotifierWatcher",
      G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT | G_BUS_NAME_OWNER_FLAGS_REPLACE,
      on_bus_acquired, on_name_acquired, on_name_lost, &state, NULL);
  status_notifier_diagnostic("serving private status notifier for %s at %s",
                             state.app_id, shared_dir);
  g_main_loop_run(state.loop);
  g_bus_unown_name(owner_id);
  g_ptr_array_free(state.status_items, TRUE);
  if (state.local_bus != NULL)
    g_object_unref(state.local_bus);
  g_object_unref(state.host_bus);
  g_dbus_node_info_unref(state.watcher_node);
  g_dbus_node_info_unref(state.item_node);
  g_dbus_node_info_unref(state.dbusmenu_node);
  g_main_loop_unref(state.loop);
  g_free(state.app_id);
  g_free(state.app_root);
  g_free(state.runtime_root);
  return 0;
}
