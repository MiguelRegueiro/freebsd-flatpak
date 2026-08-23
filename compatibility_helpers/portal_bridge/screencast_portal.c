#include "screencast_portal.h"
#include "pipewire_screencast_linker.h"
#include "portal_bridge_process.h"
#include "portal_request.h"
const char *SESSION_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.Session'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Close'/>"
    "    <signal name='Closed'>"
    "      <arg type='a{sv}' name='details'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

void free_session(SessionRecord *session) {
  if (session == NULL) {
    return;
  }
  remove_pipewire_links_for_session(session);
  if (session->host_signal_id != 0 && session->state->host_bus != NULL) {
    g_dbus_connection_signal_unsubscribe(session->state->host_bus,
                                         session->host_signal_id);
  }
  if (session->local_registration_id != 0 &&
      session->state->local_bus != NULL) {
    g_dbus_connection_unregister_object(session->state->local_bus,
                                        session->local_registration_id);
  }
  g_free(session->client_sender);
  g_free(session->local_path);
  g_free(session->host_path);
  if (session->sources != NULL) {
    g_array_free(session->sources, TRUE);
  }
  g_free(session);
}

SessionRecord *find_session(BridgeState *state, const char *local_path) {
  for (guint i = 0; i < state->screencast.sessions->len; i++) {
    SessionRecord *session = g_ptr_array_index(state->screencast.sessions, i);
    if (g_strcmp0(session->local_path, local_path) == 0) {
      return session;
    }
  }
  return NULL;
}

void close_host_session(SessionRecord *session) {
  remove_pipewire_links_for_session(session);
  if (session->closed || session->close_requested ||
      session->host_path == NULL || session->state->host_bus == NULL) {
    return;
  }
  session->close_requested = true;
  g_dbus_connection_call(session->state->host_bus,
                         "org.freedesktop.portal.Desktop", session->host_path,
                         "org.freedesktop.portal.Session", "Close", NULL, NULL,
                         G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
}

void on_host_session_closed(GDBusConnection *connection,
                            const gchar *sender_name, const gchar *object_path,
                            const gchar *interface_name,
                            const gchar *signal_name, GVariant *parameters,
                            gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  (void)interface_name;
  (void)signal_name;
  SessionRecord *session = user_data;
  if (session->closed) {
    return;
  }
  remove_pipewire_links_for_session(session);
  session->closed = true;
  GError *error = NULL;
  if (!g_dbus_connection_emit_signal(
          session->state->local_bus, session->client_sender,
          session->local_path, "org.freedesktop.portal.Session", "Closed",
          g_variant_ref(parameters), &error)) {
    log_line("emit Session.Closed to %s failed: %s", session->client_sender,
             error->message);
    g_error_free(error);
  }
}

void handle_session_method(GDBusConnection *connection, const gchar *sender,
                           const gchar *object_path,
                           const gchar *interface_name,
                           const gchar *method_name, GVariant *parameters,
                           GDBusMethodInvocation *invocation,
                           gpointer user_data) {
  (void)connection;
  (void)interface_name;
  (void)parameters;
  BridgeState *state = user_data;
  SessionRecord *session = find_session(state, object_path);
  if (session == NULL) {
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown session object");
    return;
  }
  if (g_strcmp0(sender, session->client_sender) != 0) {
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                          G_IO_ERROR_PERMISSION_DENIED,
                                          "session belongs to another client");
    return;
  }
  if (g_strcmp0(method_name, "Close") == 0) {
    close_host_session(session);
    g_dbus_method_invocation_return_value(invocation, NULL);
    return;
  }
  g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                        G_IO_ERROR_NOT_SUPPORTED,
                                        "%s is not implemented", method_name);
}

GVariant *handle_session_property(GDBusConnection *connection,
                                  const gchar *sender, const gchar *object_path,
                                  const gchar *interface_name,
                                  const gchar *property_name, GError **error,
                                  gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)user_data;
  if (g_strcmp0(property_name, "version") == 0) {
    return g_variant_new_uint32(1);
  }
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s",
              property_name);
  return NULL;
}

const GDBusInterfaceVTable SESSION_VTABLE = {
    .method_call = handle_session_method,
    .get_property = handle_session_property,
};

SessionRecord *register_session(RequestRecord *request, const char *host_path,
                                GError **error) {
  BridgeState *state = request->state;
  if (find_session(state, request->local_session_path) != NULL) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_EXISTS,
                "session already exists: %s", request->local_session_path);
    return NULL;
  }
  SessionRecord *session = g_new0(SessionRecord, 1);
  session->state = state;
  session->client_sender = g_strdup(request->client_sender);
  session->local_path = g_strdup(request->local_session_path);
  session->host_path = g_strdup(host_path);
  session->sources = g_array_new(FALSE, TRUE, sizeof(ScreenCastSource));
  GDBusInterfaceInfo *iface = g_dbus_node_info_lookup_interface(
      state->session_node, "org.freedesktop.portal.Session");
  session->local_registration_id = g_dbus_connection_register_object(
      state->local_bus, session->local_path, iface, &SESSION_VTABLE, state,
      NULL, error);
  if (session->local_registration_id == 0) {
    free_session(session);
    return NULL;
  }
  session->host_signal_id = g_dbus_connection_signal_subscribe(
      state->host_bus, "org.freedesktop.portal.Desktop",
      "org.freedesktop.portal.Session", "Closed", session->host_path, NULL,
      G_DBUS_SIGNAL_FLAGS_NONE, on_host_session_closed, session, NULL);
  g_ptr_array_add(state->screencast.sessions, session);
  log_line("mapped ScreenCast session %s -> %s", session->local_path,
           session->host_path);
  return session;
}

GVariant *rewrite_create_session_results(RequestRecord *request,
                                         guint32 response, GVariant *results,
                                         guint32 *out_response) {
  *out_response = response;
  if (response != 0) {
    return g_variant_ref(results);
  }
  const char *host_session = NULL;
  if (!g_variant_lookup(results, "session_handle", "&s", &host_session)) {
    log_line("host CreateSession response omitted session_handle");
    *out_response = 2;
    GVariantBuilder empty;
    g_variant_builder_init(&empty, G_VARIANT_TYPE_VARDICT);
    return g_variant_builder_end(&empty);
  }
  GError *error = NULL;
  if (register_session(request, host_session, &error) == NULL) {
    log_line("register local ScreenCast session failed: %s", error->message);
    g_error_free(error);
    *out_response = 2;
    GVariantBuilder empty;
    g_variant_builder_init(&empty, G_VARIANT_TYPE_VARDICT);
    return g_variant_builder_end(&empty);
  }

  GVariantBuilder out;
  g_variant_builder_init(&out, G_VARIANT_TYPE_VARDICT);
  GVariantIter iter;
  const char *key = NULL;
  GVariant *value = NULL;
  g_variant_iter_init(&iter, results);
  while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
    if (g_strcmp0(key, "session_handle") == 0) {
      g_variant_builder_add(&out, "{sv}", key,
                            g_variant_new_string(request->local_session_path));
    } else {
      g_variant_builder_add(&out, "{sv}", key, value);
    }
    g_variant_unref(value);
  }
  return g_variant_builder_end(&out);
}

void update_session_sources(SessionRecord *session, GVariant *results) {
  GVariant *streams =
      g_variant_lookup_value(results, "streams", G_VARIANT_TYPE("a(ua{sv})"));
  if (streams == NULL) {
    log_line("ScreenCast.Start response omitted streams");
    return;
  }

  remove_pipewire_links_for_session(session);
  g_array_set_size(session->sources, 0);
  GVariantIter iter;
  guint32 node_id = SPA_ID_INVALID;
  GVariant *properties = NULL;
  g_variant_iter_init(&iter, streams);
  while (g_variant_iter_next(&iter, "(u@a{sv})", &node_id, &properties)) {
    ScreenCastSource source = {
        .node_id = node_id,
        .serial = 0,
    };
    g_variant_lookup(properties, "pipewire-serial", "t", &source.serial);
    if (!session_approves_source(session, node_id)) {
      g_array_append_val(session->sources, source);
      log_line("approved ScreenCast source node %u (serial %" G_GUINT64_FORMAT
               ") for session %s",
               node_id, source.serial, session->local_path);
    }
    g_variant_unref(properties);
  }
  g_variant_unref(streams);
  refresh_pipewire_permissions_for_client(session->state->screencast.pipewire,
                                          SPA_ID_INVALID);
  pipewire_compat_try_links(session->state->screencast.pipewire);
}

void on_host_screencast_response(GDBusConnection *connection,
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
  RequestRecord *request = user_data;
  guint32 response = 2;
  GVariant *results = NULL;
  g_variant_get(parameters, "(u@a{sv})", &response, &results);
  if (request->close_requested) {
    g_variant_unref(results);
    if (request->host_signal_id != 0) {
      g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                           request->host_signal_id);
      request->host_signal_id = 0;
    }
    return;
  }
  if (response == 0 && request->kind == REQUEST_SCREENCAST_START &&
      request->session != NULL) {
    update_session_sources(request->session, results);
  }
  GVariant *forwarded = request->kind == REQUEST_SCREENCAST_CREATE
                            ? rewrite_create_session_results(request, response,
                                                             results, &response)
                            : g_variant_ref(results);
  g_variant_unref(results);
  emit_request_response(request, response, forwarded);
  if (request->host_signal_id != 0) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
    request->host_signal_id = 0;
  }
}

void subscribe_host_request(RequestRecord *request) {
  if (request->host_signal_id != 0) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
  }
  request->host_signal_id = g_dbus_connection_signal_subscribe(
      request->state->host_bus, "org.freedesktop.portal.Desktop",
      "org.freedesktop.portal.Request", "Response", request->host_path, NULL,
      G_DBUS_SIGNAL_FLAGS_NONE, on_host_screencast_response, request, NULL);
}

void on_host_screencast_call(GObject *source_object, GAsyncResult *result,
                             gpointer user_data) {
  GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
  RequestRecord *request = user_data;
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
  if (reply == NULL) {
    log_line("host ScreenCast call failed: %s", error->message);
    g_error_free(error);
    emit_cancel_response(request);
    return;
  }
  const char *actual_path = NULL;
  g_variant_get(reply, "(&o)", &actual_path);
  if (g_strcmp0(actual_path, request->host_path) != 0) {
    log_line("host returned unexpected request path %s (predicted %s)",
             actual_path, request->host_path);
    g_free(request->host_path);
    request->host_path = g_strdup(actual_path);
    subscribe_host_request(request);
  }
  if (request->close_requested) {
    g_dbus_connection_call(request->state->host_bus,
                           "org.freedesktop.portal.Desktop", request->host_path,
                           "org.freedesktop.portal.Request", "Close", NULL,
                           NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
  }
  g_variant_unref(reply);
}

RequestRecord *register_screencast_request(BridgeState *state,
                                           const char *sender,
                                           GVariant *options, RequestKind kind,
                                           const char *host_token,
                                           GError **error) {
  RequestRecord *request = g_new0(RequestRecord, 1);
  request->state = state;
  request->client_sender = g_strdup(sender);
  request->local_path = request_path_for_options(state, sender, options);
  request->kind = kind;
  const char *host_sender = g_dbus_connection_get_unique_name(state->host_bus);
  request->host_path = portal_path("request", host_sender, host_token);
  GDBusInterfaceInfo *iface = g_dbus_node_info_lookup_interface(
      state->request_node, "org.freedesktop.portal.Request");
  request->local_registration_id = g_dbus_connection_register_object(
      state->local_bus, request->local_path, iface, &REQUEST_VTABLE, state,
      NULL, error);
  if (request->local_registration_id == 0) {
    free_request(request);
    return NULL;
  }
  subscribe_host_request(request);
  g_ptr_array_add(state->request_store.requests, request);
  return request;
}

SessionRecord *owned_session(BridgeState *state, const char *sender,
                             const char *local_path,
                             GDBusMethodInvocation *invocation) {
  SessionRecord *session = find_session(state, local_path);
  if (session == NULL || session->closed || session->close_requested) {
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
        "unknown or closed session: %s", local_path);
    return NULL;
  }
  if (g_strcmp0(sender, session->client_sender) != 0) {
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                          G_IO_ERROR_PERMISSION_DENIED,
                                          "session belongs to another client");
    return NULL;
  }
  return session;
}

void handle_screencast_create(BridgeState *state, const char *sender,
                              GVariant *parameters,
                              GDBusMethodInvocation *invocation) {
  GVariant *options = g_variant_get_child_value(parameters, 0);
  char *host_handle_token = fresh_host_token(state, "request");
  char *host_session_token = fresh_host_token(state, "session");
  GError *error = NULL;
  RequestRecord *request = register_screencast_request(
      state, sender, options, REQUEST_SCREENCAST_CREATE, host_handle_token,
      &error);
  if (request == NULL) {
    g_dbus_method_invocation_take_error(invocation, error);
    goto out;
  }
  char *local_session_token = token_from_options(
      state, options, "session_handle_token", "freebsd_flatpak_session");
  request->local_session_path =
      portal_path("session", sender, local_session_token);
  g_free(local_session_token);
  GVariant *host_options =
      rewrite_options(options, host_handle_token, host_session_token);
  g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                         "/org/freedesktop/portal/desktop",
                         "org.freedesktop.portal.ScreenCast", "CreateSession",
                         g_variant_new("(@a{sv})", host_options),
                         G_VARIANT_TYPE("(o)"), G_DBUS_CALL_FLAGS_NONE, -1,
                         NULL, on_host_screencast_call, request);
  g_dbus_method_invocation_return_value(
      invocation, g_variant_new("(o)", request->local_path));
  log_line("forwarded ScreenCast.CreateSession as %s", request->local_path);
out:
  g_free(host_handle_token);
  g_free(host_session_token);
  g_variant_unref(options);
}

void handle_screencast_request(BridgeState *state, const char *sender,
                               const char *method_name, GVariant *parameters,
                               GDBusMethodInvocation *invocation) {
  const char *local_session_path = NULL;
  g_variant_get_child(parameters, 0, "&o", &local_session_path);
  SessionRecord *session =
      owned_session(state, sender, local_session_path, invocation);
  if (session == NULL) {
    return;
  }
  gsize options_index = g_strcmp0(method_name, "Start") == 0 ? 2 : 1;
  GVariant *options = g_variant_get_child_value(parameters, options_index);
  char *host_token = fresh_host_token(state, "request");
  GError *error = NULL;
  bool is_start = g_strcmp0(method_name, "Start") == 0;
  RequestRecord *request = register_screencast_request(
      state, sender, options,
      is_start ? REQUEST_SCREENCAST_START : REQUEST_SCREENCAST_OTHER,
      host_token, &error);
  if (request == NULL) {
    g_dbus_method_invocation_take_error(invocation, error);
    g_free(host_token);
    g_variant_unref(options);
    return;
  }
  request->session = session;
  GVariant *host_options = rewrite_options(options, host_token, NULL);
  GVariant *host_parameters = NULL;
  if (is_start) {
    const char *parent_window = NULL;
    g_variant_get_child(parameters, 1, "&s", &parent_window);
    host_parameters = g_variant_new("(os@a{sv})", session->host_path,
                                    parent_window, host_options);
  } else {
    host_parameters =
        g_variant_new("(o@a{sv})", session->host_path, host_options);
  }
  g_dbus_connection_call(
      state->host_bus, "org.freedesktop.portal.Desktop",
      "/org/freedesktop/portal/desktop", "org.freedesktop.portal.ScreenCast",
      method_name, host_parameters, G_VARIANT_TYPE("(o)"),
      G_DBUS_CALL_FLAGS_NONE, -1, NULL, on_host_screencast_call, request);
  g_dbus_method_invocation_return_value(
      invocation, g_variant_new("(o)", request->local_path));
  log_line("forwarded ScreenCast.%s as %s", method_name, request->local_path);
  g_free(host_token);
  g_variant_unref(options);
}

gint32 copy_unix_fd(GUnixFDList *source_fds, gint32 source_index,
                    GUnixFDList *destination_fds, GError **error) {
  if (source_fds == NULL) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                "host portal returned no Unix FD list");
    return -1;
  }
  int source_fd = g_unix_fd_list_get(source_fds, source_index, error);
  if (source_fd < 0) {
    return -1;
  }
  gint32 destination_index =
      g_unix_fd_list_append(destination_fds, source_fd, error);
  close(source_fd);
  return destination_index;
}

void on_open_pipewire_remote(GObject *source_object, GAsyncResult *result,
                             gpointer user_data) {
  GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
  GDBusMethodInvocation *invocation = user_data;
  GError *error = NULL;
  GUnixFDList *host_fds = NULL;
  GVariant *reply = g_dbus_connection_call_with_unix_fd_list_finish(
      connection, &host_fds, result, &error);
  if (reply == NULL) {
    g_dbus_method_invocation_take_error(invocation, error);
    g_object_unref(invocation);
    return;
  }
  gint32 host_index = -1;
  g_variant_get(reply, "(h)", &host_index);
  GUnixFDList *local_fds = g_unix_fd_list_new();
  gint32 local_index = copy_unix_fd(host_fds, host_index, local_fds, &error);
  if (local_index < 0) {
    g_dbus_method_invocation_take_error(invocation, error);
  } else {
    g_dbus_method_invocation_return_value_with_unix_fd_list(
        invocation, g_variant_new("(h)", local_index), local_fds);
    log_line("forwarded restricted PipeWire remote fd");
  }
  g_object_unref(local_fds);
  g_variant_unref(reply);
  if (host_fds != NULL) {
    g_object_unref(host_fds);
  }
  g_object_unref(invocation);
}

void handle_open_pipewire_remote(BridgeState *state, const char *sender,
                                 GVariant *parameters,
                                 GDBusMethodInvocation *invocation) {
  const char *local_session_path = NULL;
  GVariant *options = NULL;
  g_variant_get(parameters, "(&o@a{sv})", &local_session_path, &options);
  SessionRecord *session =
      owned_session(state, sender, local_session_path, invocation);
  if (session == NULL) {
    g_variant_unref(options);
    return;
  }
  g_dbus_connection_call_with_unix_fd_list(
      state->host_bus, "org.freedesktop.portal.Desktop",
      "/org/freedesktop/portal/desktop", "org.freedesktop.portal.ScreenCast",
      "OpenPipeWireRemote",
      g_variant_new("(o@a{sv})", session->host_path, options),
      G_VARIANT_TYPE("(h)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL,
      on_open_pipewire_remote, g_object_ref(invocation));
}
