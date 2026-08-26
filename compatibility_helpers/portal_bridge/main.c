#include "basic_desktop_portals.h"
#include "document_grant_store.h"
#include "document_grant_persistence.h"
#include "document_portal.h"
#include "pipewire_screencast_linker.h"
#include "portal_bridge_process.h"
#include "portal_request.h"
#include "sandbox_document_registration.h"
#include "screencast_portal.h"
#include "spawn_portal.h"
const char *arg_value(int argc, char **argv, const char *name) {
  for (int i = 1; i + 1 < argc; i++) {
    if (strcmp(argv[i], name) == 0) {
      return argv[i + 1];
    }
  }
  return NULL;
}

GDBusConnection *connect_to_bus_address(const char *address, GError **error) {
  return g_dbus_connection_new_for_address_sync(
      address,
      G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
          G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
      NULL, NULL, error);
}

int main(int argc, char **argv) {
  const char *app_id = arg_value(argc, argv, "--app-id");
  const char *doc_dir = arg_value(argc, argv, "--doc-dir");
  const char *sandbox_root = arg_value(argc, argv, "--sandbox-root");
  const char *mountpoint = arg_value(argc, argv, "--mountpoint");
  const char *grant_store = arg_value(argc, argv, "--grant-store");
  const char *host_bus_address = getenv("HOST_DBUS_SESSION_BUS_ADDRESS");
  if (app_id == NULL || doc_dir == NULL || sandbox_root == NULL ||
      mountpoint == NULL || grant_store == NULL || host_bus_address == NULL ||
      *host_bus_address == '\0') {
    fprintf(stderr,
            "usage: %s --app-id APP_ID --doc-dir HOST_DOC_DIR --sandbox-root "
            "APP_CHROOT_ROOT --mountpoint SANDBOX_MOUNTPOINT --grant-store "
            "PERSISTENT_GRANT_FILE\n",
            argv[0]);
    fprintf(
        stderr,
        "HOST_DBUS_SESSION_BUS_ADDRESS must point at the host session bus\n");
    return 64;
  }

  if (g_mkdir_with_parents(doc_dir, 0700) != 0) {
    fprintf(stderr, "create %s failed: %s\n", doc_dir, g_strerror(errno));
    return 1;
  }

  GError *error = NULL;
  BridgeState state = {
      .app_id = g_strdup(app_id),
      .documents =
          {
              .doc_dir = g_strdup(doc_dir),
              .sandbox_root = g_strdup(sandbox_root),
              .mountpoint = g_strdup(mountpoint),
              .persistent_store = g_strdup(grant_store),
              .sandbox_doc_dirs = g_ptr_array_new_with_free_func(g_free),
              .grants =
                  g_ptr_array_new_with_free_func((GDestroyNotify)free_grant),
          },
      .request_store =
          {
              .requests =
                  g_ptr_array_new_with_free_func((GDestroyNotify)free_request),
              .request_counter = 0,
              .host_token_counter = 0,
          },
      .screencast =
          {
              .sessions =
                  g_ptr_array_new_with_free_func((GDestroyNotify)free_session),
              .pipewire = NULL,
          },
      .loop = g_main_loop_new(NULL, FALSE),
      .host_bus = connect_to_bus_address(host_bus_address, &error),
      .local_bus = NULL,
      .desktop_node = g_dbus_node_info_new_for_xml(DESKTOP_XML, &error),
      .documents_node = NULL,
      .request_node = NULL,
      .session_node = NULL,
      .control_node = NULL,
      .flatpak_node = NULL,
  };
  if (state.host_bus == NULL || state.desktop_node == NULL) {
    fprintf(stderr, "portal bridge setup failed: %s\n", error->message);
    g_error_free(error);
    return 1;
  }
  if (!load_persistent_document_grants(&state, &error)) {
    fprintf(stderr, "load persistent document grants failed: %s\n",
            error->message);
    g_error_free(error);
    return 1;
  }
  state.documents_node = g_dbus_node_info_new_for_xml(DOCUMENTS_XML, &error);
  state.request_node = g_dbus_node_info_new_for_xml(REQUEST_XML, &error);
  state.session_node = g_dbus_node_info_new_for_xml(SESSION_XML, &error);
  state.control_node = g_dbus_node_info_new_for_xml(CONTROL_XML, &error);
  state.flatpak_node =
      g_dbus_node_info_new_for_xml(FLATPAK_PORTAL_XML, &error);
  if (state.documents_node == NULL || state.request_node == NULL ||
      state.session_node == NULL || state.control_node == NULL ||
      state.flatpak_node == NULL) {
    fprintf(stderr, "portal bridge introspection failed: %s\n", error->message);
    g_error_free(error);
    return 1;
  }
  portal_bridge_process_load_host_properties(&state);
  spawn_portal_initialize(&state);
  state.screencast.pipewire = new_pipewire_compat(&state);
  if (state.screencast.pipewire == NULL) {
    log_line("PipeWire compatibility linking unavailable; ScreenCast "
             "forwarding will continue");
  }

  g_unix_signal_add(SIGINT, portal_bridge_process_handle_signal, &state);
  g_unix_signal_add(SIGTERM, portal_bridge_process_handle_signal, &state);

  guint desktop_owner_id = g_bus_own_name(
      G_BUS_TYPE_SESSION, "org.freedesktop.portal.Desktop",
      G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT | G_BUS_NAME_OWNER_FLAGS_REPLACE,
      portal_bridge_process_on_bus_acquired,
      portal_bridge_process_on_name_acquired,
      portal_bridge_process_on_name_lost, &state, NULL);
  guint documents_owner_id = g_bus_own_name(
      G_BUS_TYPE_SESSION, "org.freedesktop.portal.Documents",
      G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT | G_BUS_NAME_OWNER_FLAGS_REPLACE,
      portal_bridge_process_on_bus_acquired,
      portal_bridge_process_on_name_acquired,
      portal_bridge_process_on_name_lost, &state, NULL);
  guint flatpak_owner_id = g_bus_own_name(
      G_BUS_TYPE_SESSION, FLATPAK_PORTAL_BUS_NAME,
      G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT | G_BUS_NAME_OWNER_FLAGS_REPLACE,
      portal_bridge_process_on_bus_acquired,
      portal_bridge_process_on_name_acquired,
      portal_bridge_process_on_name_lost, &state, NULL);
  log_line("serving private portal for %s at %s", state.app_id,
           state.documents.doc_dir);
  g_main_loop_run(state.loop);

  portal_bridge_process_cleanup_documents(&state);
  for (guint i = 0; i < state.request_store.requests->len; i++) {
    RequestRecord *request = g_ptr_array_index(state.request_store.requests, i);
    if (!request->completed && request->host_path != NULL) {
      g_dbus_connection_call(
          state.host_bus, "org.freedesktop.portal.Desktop", request->host_path,
          "org.freedesktop.portal.Request", "Close", NULL, NULL,
          G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
    }
  }
  for (guint i = 0; i < state.screencast.sessions->len; i++) {
    close_host_session(g_ptr_array_index(state.screencast.sessions, i));
  }
  g_dbus_connection_flush_sync(state.host_bus, NULL, NULL);
  if (state.local_name_signal_id != 0 && state.local_bus != NULL) {
    g_dbus_connection_signal_unsubscribe(state.local_bus,
                                         state.local_name_signal_id);
    state.local_name_signal_id = 0;
  }
  free_pipewire_compat(state.screencast.pipewire);
  state.screencast.pipewire = NULL;
  g_ptr_array_free(state.screencast.sessions, TRUE);
  state.screencast.sessions = NULL;
  g_ptr_array_free(state.request_store.requests, TRUE);
  state.request_store.requests = NULL;
  spawn_portal_cleanup(&state);
  g_bus_unown_name(flatpak_owner_id);
  g_bus_unown_name(documents_owner_id);
  g_bus_unown_name(desktop_owner_id);
  if (state.local_bus != NULL) {
    g_object_unref(state.local_bus);
  }
  if (state.host_bus != NULL) {
    g_object_unref(state.host_bus);
  }
  g_dbus_node_info_unref(state.desktop_node);
  g_dbus_node_info_unref(state.documents_node);
  g_dbus_node_info_unref(state.request_node);
  g_dbus_node_info_unref(state.session_node);
  g_dbus_node_info_unref(state.control_node);
  g_dbus_node_info_unref(state.flatpak_node);
  g_main_loop_unref(state.loop);
  g_ptr_array_free(state.documents.grants, TRUE);
  g_ptr_array_free(state.documents.sandbox_doc_dirs, TRUE);
  g_free(state.app_id);
  g_free(state.documents.doc_dir);
  g_free(state.documents.sandbox_root);
  g_free(state.documents.mountpoint);
  g_free(state.documents.persistent_store);
  return 0;
}
