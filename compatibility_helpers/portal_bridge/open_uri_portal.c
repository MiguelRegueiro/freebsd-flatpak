#include "open_uri_portal.h"
#include "portal_request.h"

typedef struct {
  RequestRecord *request;
  GDBusMethodInvocation *invocation;
} OpenUriCall;

static void subscribe_open_uri_response(RequestRecord *request);

GVariant *open_uri_host_parameters(const char *method_name,
                                   GVariant *parameters,
                                   const char *host_token, gint32 host_fd) {
  const char *parent_window = NULL;
  GVariant *options = g_variant_get_child_value(parameters, 2);
  GVariant *host_options = rewrite_options(options, host_token, NULL);
  g_variant_unref(options);
  g_variant_get_child(parameters, 0, "&s", &parent_window);
  if (g_strcmp0(method_name, "OpenURI") == 0) {
    const char *uri = NULL;
    g_variant_get_child(parameters, 1, "&s", &uri);
    return g_variant_new("(ss@a{sv})", parent_window, uri, host_options);
  }
  return g_variant_new("(sh@a{sv})", parent_window, host_fd, host_options);
}

static void on_open_uri_response(GDBusConnection *connection,
                                 const gchar *sender_name,
                                 const gchar *object_path,
                                 const gchar *interface_name,
                                 const gchar *signal_name,
                                 GVariant *parameters, gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  (void)interface_name;
  (void)signal_name;
  RequestRecord *request = user_data;
  guint32 response = 2;
  GVariant *results = NULL;
  g_variant_get(parameters, "(u@a{sv})", &response, &results);
  if (request->close_requested) {
    g_variant_unref(results);
  } else {
    emit_request_response(request, response, results);
  }
  if (request->host_signal_id != 0) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
    request->host_signal_id = 0;
  }
}

static void subscribe_open_uri_response(RequestRecord *request) {
  if (request->host_signal_id != 0) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
  }
  request->host_signal_id = g_dbus_connection_signal_subscribe(
      request->state->host_bus, "org.freedesktop.portal.Desktop",
      "org.freedesktop.portal.Request", "Response", request->host_path, NULL,
      G_DBUS_SIGNAL_FLAGS_NONE, on_open_uri_response, request, NULL);
}

static RequestRecord *register_open_uri_request(BridgeState *state,
                                                const char *sender,
                                                GVariant *options,
                                                const char *host_token,
                                                GError **error) {
  RequestRecord *request = g_new0(RequestRecord, 1);
  request->state = state;
  request->client_sender = g_strdup(sender);
  request->local_path = request_path_for_options(state, sender, options);
  request->kind = REQUEST_OPEN_URI;
  request->host_path = portal_path(
      "request", g_dbus_connection_get_unique_name(state->host_bus), host_token);
  GDBusInterfaceInfo *iface = g_dbus_node_info_lookup_interface(
      state->request_node, "org.freedesktop.portal.Request");
  request->local_registration_id = g_dbus_connection_register_object(
      state->local_bus, request->local_path, iface, &REQUEST_VTABLE, state,
      NULL, error);
  if (request->local_registration_id == 0) {
    free_request(request);
    return NULL;
  }
  subscribe_open_uri_response(request);
  g_ptr_array_add(state->request_store.requests, request);
  return request;
}

static void finish_open_uri_call(OpenUriCall *call, GVariant *reply,
                                 GError *error) {
  RequestRecord *request = call->request;
  if (reply == NULL) {
    request->completed = true;
    if (request->host_signal_id != 0) {
      g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                           request->host_signal_id);
      request->host_signal_id = 0;
    }
    g_dbus_method_invocation_take_error(call->invocation, error);
  } else {
    const char *actual_path = NULL;
    g_variant_get(reply, "(&o)", &actual_path);
    if (g_strcmp0(actual_path, request->host_path) != 0) {
      g_free(request->host_path);
      request->host_path = g_strdup(actual_path);
      subscribe_open_uri_response(request);
    }
    if (request->close_requested) {
      g_dbus_connection_call(request->state->host_bus,
                             "org.freedesktop.portal.Desktop",
                             request->host_path,
                             "org.freedesktop.portal.Request", "Close", NULL,
                             NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
    }
    g_dbus_method_invocation_return_value(
        call->invocation, g_variant_new("(o)", request->local_path));
    g_variant_unref(reply);
  }
  g_object_unref(call->invocation);
  g_free(call);
}

static void on_open_uri_call(GObject *source_object, GAsyncResult *result,
                             gpointer user_data) {
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_finish(
      G_DBUS_CONNECTION(source_object), result, &error);
  finish_open_uri_call(user_data, reply, error);
}

static void on_open_uri_fd_call(GObject *source_object, GAsyncResult *result,
                                gpointer user_data) {
  GError *error = NULL;
  GUnixFDList *reply_fds = NULL;
  GVariant *reply = g_dbus_connection_call_with_unix_fd_list_finish(
      G_DBUS_CONNECTION(source_object), &reply_fds, result, &error);
  if (reply_fds != NULL) {
    g_object_unref(reply_fds);
  }
  finish_open_uri_call(user_data, reply, error);
}

GUnixFDList *copy_open_uri_fd(GUnixFDList *source_fds, gint32 source_index,
                                   gint32 *destination_index,
                                   GError **error) {
  if (source_fds == NULL) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                "OpenURI call did not include a Unix FD list");
    return NULL;
  }
  int source_fd = g_unix_fd_list_get(source_fds, source_index, error);
  if (source_fd < 0) {
    return NULL;
  }
  GUnixFDList *destination_fds = g_unix_fd_list_new();
  *destination_index =
      g_unix_fd_list_append(destination_fds, source_fd, error);
  close(source_fd);
  if (*destination_index < 0) {
    g_object_unref(destination_fds);
    return NULL;
  }
  return destination_fds;
}

void handle_open_uri_request(BridgeState *state, const char *sender,
                             const char *method_name, GVariant *parameters,
                             GDBusMethodInvocation *invocation) {
  GError *error = NULL;
  gint32 host_fd = -1;
  GUnixFDList *host_fds = NULL;
  bool forwards_fd = g_strcmp0(method_name, "OpenURI") != 0;
  if (forwards_fd) {
    gint32 source_fd = -1;
    g_variant_get_child(parameters, 1, "h", &source_fd);
    GDBusMessage *message = g_dbus_method_invocation_get_message(invocation);
    GUnixFDList *source_fds = g_dbus_message_get_unix_fd_list(message);
    host_fds = copy_open_uri_fd(source_fds, source_fd, &host_fd, &error);
    if (host_fds == NULL) {
      g_dbus_method_invocation_take_error(invocation, error);
      return;
    }
  }

  GVariant *options = g_variant_get_child_value(parameters, 2);
  char *host_token = fresh_host_token(state, "open_uri");
  RequestRecord *request = register_open_uri_request(
      state, sender, options, host_token, &error);
  if (request == NULL) {
    if (host_fds != NULL) {
      g_object_unref(host_fds);
    }
    g_variant_unref(options);
    g_free(host_token);
    g_dbus_method_invocation_take_error(invocation, error);
    return;
  }
  GVariant *host_parameters =
      open_uri_host_parameters(method_name, parameters, host_token, host_fd);
  OpenUriCall *call = g_new0(OpenUriCall, 1);
  call->request = request;
  call->invocation = g_object_ref(invocation);
  if (forwards_fd) {
    g_dbus_connection_call_with_unix_fd_list(
        state->host_bus, "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop", "org.freedesktop.portal.OpenURI",
        method_name, host_parameters, G_VARIANT_TYPE("(o)"),
        G_DBUS_CALL_FLAGS_NONE, -1, host_fds, NULL, on_open_uri_fd_call, call);
    g_object_unref(host_fds);
  } else {
    g_dbus_connection_call(
        state->host_bus, "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop", "org.freedesktop.portal.OpenURI",
        method_name, host_parameters, G_VARIANT_TYPE("(o)"),
        G_DBUS_CALL_FLAGS_NONE, -1, NULL, on_open_uri_call, call);
  }
  diagnostic_line("forwarding OpenURI.%s as %s", method_name,
                  request->local_path);
  g_variant_unref(options);
  g_free(host_token);
}
