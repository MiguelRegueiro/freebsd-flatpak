#include "portal_request.h"
#include "file_chooser_portal.h"
#include "portal_bridge_process.h"
const char *REQUEST_XML = "<node>"
                          "  <interface name='org.freedesktop.portal.Request'>"
                          "    <method name='Close'/>"
                          "    <signal name='Response'>"
                          "      <arg type='u' name='response'/>"
                          "      <arg type='a{sv}' name='results'/>"
                          "    </signal>"
                          "  </interface>"
                          "</node>";

void free_request(RequestRecord *request) {
  if (request == NULL) {
    return;
  }
  if (request->host_signal_id != 0 && request->state->host_bus != NULL) {
    g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                         request->host_signal_id);
  }
  if (request->local_registration_id != 0 &&
      request->state->local_bus != NULL) {
    g_dbus_connection_unregister_object(request->state->local_bus,
                                        request->local_registration_id);
  }
  g_free(request->client_sender);
  g_free(request->local_path);
  g_free(request->host_path);
  g_free(request->local_session_path);
  g_free(request);
}

char *safe_path_element(const char *input) {
  GString *value = g_string_new("");
  for (const char *p = input; p != NULL && *p != '\0'; p++) {
    if (g_ascii_isalnum(*p) || *p == '_') {
      g_string_append_c(value, *p);
    } else {
      g_string_append_c(value, '_');
    }
  }
  if (value->len == 0) {
    g_string_append(value, "x");
  }
  return g_string_free(value, FALSE);
}

char *sender_path_element(const char *sender) {
  if (sender != NULL && sender[0] == ':') {
    sender++;
  }
  return safe_path_element(sender);
}

char *portal_path(const char *kind, const char *sender, const char *token) {
  char *sender_element = sender_path_element(sender);
  char *token_element = safe_path_element(token);
  char *path = g_strdup_printf("/org/freedesktop/portal/desktop/%s/%s/%s", kind,
                               sender_element, token_element);
  g_free(sender_element);
  g_free(token_element);
  return path;
}

RequestRecord *find_request(BridgeState *state, const char *local_path) {
  for (guint i = 0; i < state->request_store.requests->len; i++) {
    RequestRecord *request =
        g_ptr_array_index(state->request_store.requests, i);
    if (g_strcmp0(request->local_path, local_path) == 0) {
      return request;
    }
  }
  return NULL;
}

void emit_request_response(RequestRecord *request, guint32 response,
                           GVariant *results) {
  if (request->completed) {
    g_variant_unref(results);
    return;
  }
  request->completed = true;
  GError *error = NULL;
  if (!g_dbus_connection_emit_signal(
          request->state->local_bus, request->client_sender,
          request->local_path, "org.freedesktop.portal.Request", "Response",
          g_variant_new("(u@a{sv})", response, results), &error)) {
    log_line("emit Response to %s failed: %s", request->client_sender,
             error->message);
    g_error_free(error);
    return;
  }
  diagnostic_line("emitted Response %u to %s on %s", response,
                  request->client_sender, request->local_path);
}

void emit_cancel_response(RequestRecord *request) {
  GVariantBuilder results;
  g_variant_builder_init(&results, G_VARIANT_TYPE("a{sv}"));
  emit_request_response(request, 2, g_variant_builder_end(&results));
}

GVariant *option_value(GVariant *options, const char *key) {
  GVariantIter iter;
  const char *name = NULL;
  GVariant *value = NULL;
  g_variant_iter_init(&iter, options);
  while (g_variant_iter_next(&iter, "{&sv}", &name, &value)) {
    if (g_strcmp0(name, key) == 0) {
      return value;
    }
    g_variant_unref(value);
  }
  return NULL;
}

char *token_from_options(BridgeState *state, GVariant *options, const char *key,
                         const char *fallback) {
  GVariant *token_value = option_value(options, key);
  char *token = NULL;
  if (token_value != NULL &&
      g_variant_is_of_type(token_value, G_VARIANT_TYPE_STRING)) {
    token = safe_path_element(g_variant_get_string(token_value, NULL));
    g_variant_unref(token_value);
  } else {
    if (token_value != NULL) {
      g_variant_unref(token_value);
    }
    token = g_strdup_printf("%s_%" G_GUINT64_FORMAT, fallback,
                            ++state->request_store.request_counter);
  }
  return token;
}

char *request_path_for_options(BridgeState *state, const char *sender,
                               GVariant *options) {
  char *token =
      token_from_options(state, options, "handle_token", "freebsd_flatpak");
  char *path = portal_path("request", sender, token);
  g_free(token);
  return path;
}

char *request_path_for_call(BridgeState *state, const char *sender,
                            GVariant *parameters, gsize options_index) {
  GVariant *options = g_variant_get_child_value(parameters, options_index);
  char *path = request_path_for_options(state, sender, options);
  g_variant_unref(options);
  return path;
}

char *fresh_host_token(BridgeState *state, const char *label) {
  return g_strdup_printf("freebsd_flatpak_%s_%" G_GUINT64_FORMAT, label,
                         ++state->request_store.host_token_counter);
}

GVariant *rewrite_options(GVariant *options, const char *handle_token,
                          const char *session_token) {
  GVariantBuilder out;
  g_variant_builder_init(&out, G_VARIANT_TYPE_VARDICT);
  GVariantIter iter;
  const char *key = NULL;
  GVariant *value = NULL;
  g_variant_iter_init(&iter, options);
  while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
    if (g_strcmp0(key, "handle_token") != 0 &&
        (session_token == NULL ||
         g_strcmp0(key, "session_handle_token") != 0)) {
      g_variant_builder_add(&out, "{sv}", key, value);
    }
    g_variant_unref(value);
  }
  if (handle_token != NULL) {
    g_variant_builder_add(&out, "{sv}", "handle_token",
                          g_variant_new_string(handle_token));
  }
  if (session_token != NULL) {
    g_variant_builder_add(&out, "{sv}", "session_handle_token",
                          g_variant_new_string(session_token));
  }
  return g_variant_builder_end(&out);
}

void handle_request_method(GDBusConnection *connection, const gchar *sender,
                           const gchar *object_path,
                           const gchar *interface_name,
                           const gchar *method_name, GVariant *parameters,
                           GDBusMethodInvocation *invocation,
                           gpointer user_data) {
  (void)connection;
  (void)interface_name;
  (void)parameters;

  BridgeState *state = user_data;
  RequestRecord *request = find_request(state, object_path);
  if (request == NULL) {
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown request object");
    return;
  }
  if (g_strcmp0(sender, request->client_sender) != 0) {
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                          G_IO_ERROR_PERMISSION_DENIED,
                                          "request belongs to another client");
    return;
  }
  if (g_strcmp0(method_name, "Close") == 0) {
    request->close_requested = true;
    request->completed = true;
    if (request->host_path != NULL) {
      g_dbus_connection_call(
          request->state->host_bus, "org.freedesktop.portal.Desktop",
          request->host_path, "org.freedesktop.portal.Request", "Close", NULL,
          NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
    }
    g_dbus_method_invocation_return_value(invocation, NULL);
    return;
  }
  g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                        G_IO_ERROR_NOT_SUPPORTED,
                                        "%s is not implemented", method_name);
}

GVariant *handle_request_property(GDBusConnection *connection,
                                  const gchar *sender, const gchar *object_path,
                                  const gchar *interface_name,
                                  const gchar *property_name, GError **error,
                                  gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)user_data;
  g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s",
              property_name);
  return NULL;
}

const GDBusInterfaceVTable REQUEST_VTABLE = {
    .method_call = handle_request_method,
    .get_property = handle_request_property,
};
