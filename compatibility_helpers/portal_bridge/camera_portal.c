#include "camera_portal.h"
#include "pipewire_screencast_linker.h"
#include "portal_request.h"
#include "screencast_portal.h"

bool camera_sender_is_allowed(BridgeState *state, const char *sender) {
  return state->camera.allowed_senders != NULL &&
         g_hash_table_contains(state->camera.allowed_senders, sender);
}
void camera_portal_allow_sender(BridgeState *state, const char *sender) {
  g_hash_table_add(state->camera.allowed_senders, g_strdup(sender));
}
void camera_portal_forget_sender(BridgeState *state, const char *sender) {
  if (state->camera.allowed_senders != NULL) {
    g_hash_table_remove(state->camera.allowed_senders, sender);
  }
}
void camera_portal_apply_response(RequestRecord *request, guint32 response) {
  if (!request->close_requested && response == 0) {
    camera_portal_allow_sender(request->state, request->client_sender);
  }
}
void on_host_camera_response(GDBusConnection *connection,
                             const gchar *sender_name, const gchar *object_path,
                             const gchar *interface_name,
                             const gchar *signal_name, GVariant *parameters,
                             gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  (void)interface_name;
  (void)signal_name;
  RequestRecord *request = user_data;
  guint32 response = 2;
  GVariant *results = NULL;
  g_variant_get(parameters, "(u@a{sv})", &response, &results);
  if (!request->close_requested) {
    camera_portal_apply_response(request, response);
    emit_request_response(request, response, g_variant_ref(results));
  }
  g_variant_unref(results);
  if (request->host_signal_id != 0) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
    request->host_signal_id = 0;
  }
}
void subscribe_host_camera_request(RequestRecord *request) {
  if (request->host_signal_id != 0) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
  }
  request->host_signal_id = g_dbus_connection_signal_subscribe(
      request->state->host_bus, "org.freedesktop.portal.Desktop",
      "org.freedesktop.portal.Request", "Response", request->host_path, NULL,
      G_DBUS_SIGNAL_FLAGS_NONE, on_host_camera_response, request, NULL);
}
void on_host_camera_access_call(GObject *source_object, GAsyncResult *result,
                                gpointer user_data) {
  GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
  RequestRecord *request = user_data;
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
  if (reply == NULL) {
    log_line("host Camera.AccessCamera call failed: %s", error->message);
    g_error_free(error);
    emit_cancel_response(request);
    return;
  }
  const char *actual_path = NULL;
  g_variant_get(reply, "(&o)", &actual_path);
  if (g_strcmp0(actual_path, request->host_path) != 0) {
    g_free(request->host_path);
    request->host_path = g_strdup(actual_path);
    subscribe_host_camera_request(request);
  }
  if (request->close_requested) {
    g_dbus_connection_call(request->state->host_bus,
                           "org.freedesktop.portal.Desktop", request->host_path,
                           "org.freedesktop.portal.Request", "Close", NULL,
                           NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
  }
  g_variant_unref(reply);
}
void handle_camera_access(BridgeState *state, const char *sender,
                          GVariant *parameters,
                          GDBusMethodInvocation *invocation) {
  GVariant *options = g_variant_get_child_value(parameters, 0);
  char *host_token = fresh_host_token(state, "camera");
  pipewire_request_camera_publication(state->screencast.pipewire);

  RequestRecord *request = g_new0(RequestRecord, 1);
  request->state = state;
  request->client_sender = g_strdup(sender);
  request->local_path = request_path_for_options(state, sender, options);
  request->kind = REQUEST_CAMERA_ACCESS;
  const char *host_sender = g_dbus_connection_get_unique_name(state->host_bus);
  request->host_path = portal_path("request", host_sender, host_token);
  GDBusInterfaceInfo *iface = g_dbus_node_info_lookup_interface(
      state->request_node, "org.freedesktop.portal.Request");
  GError *error = NULL;
  request->local_registration_id = g_dbus_connection_register_object(
      state->local_bus, request->local_path, iface, &REQUEST_VTABLE, state,
      NULL, &error);
  if (request->local_registration_id == 0) {
    g_dbus_method_invocation_take_error(invocation, error);
    free_request(request);
    g_free(host_token);
    g_variant_unref(options);
    return;
  }
  subscribe_host_camera_request(request);
  g_ptr_array_add(state->request_store.requests, request);
  GVariant *host_options = rewrite_options(options, host_token, NULL);
  g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                         "/org/freedesktop/portal/desktop",
                         "org.freedesktop.portal.Camera", "AccessCamera",
                         g_variant_new("(@a{sv})", host_options),
                         G_VARIANT_TYPE("(o)"), G_DBUS_CALL_FLAGS_NONE, -1,
                         NULL, on_host_camera_access_call, request);
  g_dbus_method_invocation_return_value(
      invocation, g_variant_new("(o)", request->local_path));
  diagnostic_line("forwarded Camera.AccessCamera as %s", request->local_path);
  g_free(host_token);
  g_variant_unref(options);
}
void handle_camera_open_remote(BridgeState *state, const char *sender,
                               GVariant *parameters,
                               GDBusMethodInvocation *invocation) {
  if (!camera_sender_is_allowed(state, sender)) {
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED,
        "Camera.AccessCamera has not succeeded for this client");
    return;
  }
  GVariant *options = g_variant_get_child_value(parameters, 0);
  g_dbus_connection_call_with_unix_fd_list(
      state->host_bus, "org.freedesktop.portal.Desktop",
      "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Camera",
      "OpenPipeWireRemote", g_variant_new("(@a{sv})", options),
      G_VARIANT_TYPE("(h)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL,
      on_open_pipewire_remote, g_object_ref(invocation));
}
void handle_camera_method(BridgeState *state, const char *sender,
                          const char *method_name, GVariant *parameters,
                          GDBusMethodInvocation *invocation) {
  if (g_strcmp0(method_name, "AccessCamera") == 0) {
    handle_camera_access(state, sender, parameters, invocation);
  } else if (g_strcmp0(method_name, "OpenPipeWireRemote") == 0) {
    handle_camera_open_remote(state, sender, parameters, invocation);
  } else {
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
        "Camera.%s is not implemented by this V1 bridge", method_name);
  }
}
