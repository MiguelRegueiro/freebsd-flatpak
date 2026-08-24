#include "portal_bridge_process.h"
#include "basic_desktop_portals.h"
#include "document_grant_store.h"
#include "document_mounts.h"
#include "document_portal.h"
#include "pipewire_screencast_linker.h"
#include "portal_request.h"
#include "sandbox_document_registration.h"
#include "screencast_portal.h"
void log_line(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  fputs("portal bridge: ", stderr);
  vfprintf(stderr, fmt, ap);
  fputc('\n', stderr);
  va_end(ap);
}

void portal_bridge_process_cleanup_documents(BridgeState *state) {
  for (guint i = 0; i < state->documents.grants->len; i++) {
    cleanup_grant(g_ptr_array_index(state->documents.grants, i));
  }
  g_ptr_array_set_size(state->documents.grants, 0);
}

gboolean portal_bridge_process_handle_signal(gpointer user_data) {
  BridgeState *state = user_data;
  portal_bridge_process_cleanup_documents(state);
  if (state->loop != NULL) {
    g_main_loop_quit(state->loop);
  }
  return G_SOURCE_REMOVE;
}

void portal_bridge_process_load_host_properties(BridgeState *state) {
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_sync(
      state->host_bus, "org.freedesktop.portal.Desktop",
      "/org/freedesktop/portal/desktop", "org.freedesktop.DBus.Properties",
      "GetAll", g_variant_new("(s)", "org.freedesktop.portal.ScreenCast"),
      G_VARIANT_TYPE("(a{sv})"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
  if (reply == NULL) {
    log_line("read host ScreenCast properties failed: %s", error->message);
    g_error_free(error);
    return;
  }

  GVariant *properties = NULL;
  g_variant_get(reply, "(@a{sv})", &properties);
  g_variant_lookup(properties, "version", "u", &state->screencast.version);
  g_variant_lookup(properties, "AvailableSourceTypes", "u",
                   &state->screencast.source_types);
  g_variant_lookup(properties, "AvailableCursorModes", "u",
                   &state->screencast.cursor_modes);
  log_line("host ScreenCast version=%u source-types=%u cursor-modes=%u",
           state->screencast.version, state->screencast.source_types,
           state->screencast.cursor_modes);
  g_variant_unref(properties);
  g_variant_unref(reply);
}

void portal_bridge_process_close_client_resources(BridgeState *state,
                                                  const char *client_sender) {
  for (guint i = 0; i < state->request_store.requests->len; i++) {
    RequestRecord *request =
        g_ptr_array_index(state->request_store.requests, i);
    if (request->completed ||
        g_strcmp0(request->client_sender, client_sender) != 0) {
      continue;
    }
    request->close_requested = true;
    request->completed = true;
    if (request->host_path != NULL) {
      g_dbus_connection_call(
          state->host_bus, "org.freedesktop.portal.Desktop", request->host_path,
          "org.freedesktop.portal.Request", "Close", NULL, NULL,
          G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
    }
  }
  for (guint i = 0; i < state->screencast.sessions->len; i++) {
    SessionRecord *session = g_ptr_array_index(state->screencast.sessions, i);
    if (g_strcmp0(session->client_sender, client_sender) == 0) {
      close_host_session(session);
    }
  }
}

void on_local_name_owner_changed(GDBusConnection *connection,
                                 const gchar *sender_name,
                                 const gchar *object_path,
                                 const gchar *interface_name,
                                 const gchar *signal_name, GVariant *parameters,
                                 gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  (void)interface_name;
  (void)signal_name;
  const char *name = NULL;
  const char *old_owner = NULL;
  const char *new_owner = NULL;
  g_variant_get(parameters, "(&s&s&s)", &name, &old_owner, &new_owner);
  if (name[0] == ':' && old_owner[0] != '\0' && new_owner[0] == '\0') {
    portal_bridge_process_close_client_resources(user_data, name);
  }
}

bool portal_bridge_process_register_interfaces(
    GDBusConnection *connection, const char *path, GDBusNodeInfo *node,
    const GDBusInterfaceVTable *vtable, BridgeState *state, GError **error) {
  for (guint i = 0; node->interfaces[i] != NULL; i++) {
    guint id = g_dbus_connection_register_object(
        connection, path, node->interfaces[i], vtable, state, NULL, error);
    if (id == 0) {
      return false;
    }
  }
  return true;
}

void portal_bridge_process_on_bus_acquired(GDBusConnection *connection,
                                           const gchar *name,
                                           gpointer user_data) {
  (void)name;
  BridgeState *state = user_data;
  if (state->local_bus == NULL) {
    state->local_bus = g_object_ref(connection);
  }
  if (state->local_objects_registered) {
    return;
  }

  state->local_name_signal_id = g_dbus_connection_signal_subscribe(
      connection, "org.freedesktop.DBus", "org.freedesktop.DBus",
      "NameOwnerChanged", "/org/freedesktop/DBus", NULL,
      G_DBUS_SIGNAL_FLAGS_NONE, on_local_name_owner_changed, state, NULL);

  GError *error = NULL;
  if (!portal_bridge_process_register_interfaces(
          connection, "/org/freedesktop/portal/desktop", state->desktop_node,
          &DESKTOP_VTABLE, state, &error)) {
    log_line("register desktop portal failed: %s", error->message);
    g_error_free(error);
    g_main_loop_quit(state->loop);
    return;
  }
  if (!portal_bridge_process_register_interfaces(
          connection, "/org/freedesktop/portal/documents",
          state->documents_node, &DOCUMENTS_VTABLE, state, &error)) {
    log_line("register documents portal failed: %s", error->message);
    g_error_free(error);
    g_main_loop_quit(state->loop);
    return;
  }
  if (!portal_bridge_process_register_interfaces(
          connection, "/org/freebsd/Flatpak/PortalBridge", state->control_node,
          &CONTROL_VTABLE, state, &error)) {
    log_line("register sandbox control failed: %s", error->message);
    g_error_free(error);
    g_main_loop_quit(state->loop);
    return;
  }
  state->local_objects_registered = true;
}

void portal_bridge_process_on_name_acquired(GDBusConnection *connection,
                                            const gchar *name,
                                            gpointer user_data) {
  (void)connection;
  (void)user_data;
  log_line("acquired %s", name);
}

void portal_bridge_process_on_name_lost(GDBusConnection *connection,
                                        const gchar *name, gpointer user_data) {
  (void)connection;
  BridgeState *state = user_data;
  log_line("lost %s", name);
  g_main_loop_quit(state->loop);
}
