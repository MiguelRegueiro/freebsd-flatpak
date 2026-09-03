#include "../compatibility_helpers/portal_bridge/basic_desktop_portals.h"
#include "../compatibility_helpers/portal_bridge/open_uri_portal.h"
#include "../compatibility_helpers/portal_bridge/portal_request.h"

void log_line(const char *format, ...) { (void)format; }
void diagnostic_line(const char *format, ...) { (void)format; }
void handle_filechooser_open(BridgeState *state, const char *sender,
                             GVariant *parameters,
                             GDBusMethodInvocation *invocation) {
  (void)state;
  (void)sender;
  (void)parameters;
  (void)invocation;
}
bool pipewire_camera_available(const PipeWireCompat *compat) {
  (void)compat;
  return false;
}
void handle_camera_method(BridgeState *state, const char *sender,
                          const char *method_name, GVariant *parameters,
                          GDBusMethodInvocation *invocation) {
  (void)state;
  (void)sender;
  (void)method_name;
  (void)parameters;
  (void)invocation;
}
void handle_screencast_create(BridgeState *state, const char *sender,
                              GVariant *parameters,
                              GDBusMethodInvocation *invocation) {
  (void)state;
  (void)sender;
  (void)parameters;
  (void)invocation;
}
void handle_screencast_request(BridgeState *state, const char *sender,
                               const char *method_name, GVariant *parameters,
                               GDBusMethodInvocation *invocation) {
  (void)state;
  (void)sender;
  (void)method_name;
  (void)parameters;
  (void)invocation;
}
void handle_open_pipewire_remote(BridgeState *state, const char *sender,
                                 GVariant *parameters,
                                 GDBusMethodInvocation *invocation) {
  (void)state;
  (void)sender;
  (void)parameters;
  (void)invocation;
}

static GVariant *test_options(void) {
  GVariantBuilder options;
  g_variant_builder_init(&options, G_VARIANT_TYPE_VARDICT);
  g_variant_builder_add(&options, "{sv}", "handle_token",
                        g_variant_new_string("sandbox-token"));
  g_variant_builder_add(&options, "{sv}", "writable",
                        g_variant_new_boolean(TRUE));
  return g_variant_builder_end(&options);
}

static void test_open_uri_introspection(void) {
  GError *error = NULL;
  GDBusNodeInfo *node = g_dbus_node_info_new_for_xml(DESKTOP_XML, &error);
  g_assert_no_error(error);
  GDBusInterfaceInfo *interface =
      g_dbus_node_info_lookup_interface(node, "org.freedesktop.portal.OpenURI");
  g_assert_nonnull(interface);
  const char *methods[] = {"OpenURI", "OpenFile", "OpenDirectory",
                           "SchemeSupported"};
  for (gsize i = 0; i < G_N_ELEMENTS(methods); i++) {
    g_assert_nonnull(
        g_dbus_interface_info_lookup_method(interface, methods[i]));
  }
  g_assert_nonnull(g_dbus_interface_info_lookup_property(interface, "version"));
  g_dbus_node_info_unref(node);
}

static void test_open_uri_parameter_forwarding(void) {
  GVariant *parameters = g_variant_ref_sink(
      g_variant_new("(ss@a{sv})", "wayland:window",
                    "https://example.com/path?x=1", test_options()));
  GVariant *forwarded = g_variant_ref_sink(
      open_uri_host_parameters("OpenURI", parameters, "host-token", -1));
  const char *parent = NULL;
  const char *uri = NULL;
  GVariant *options = NULL;
  g_variant_get(forwarded, "(&s&s@a{sv})", &parent, &uri, &options);
  const char *token = NULL;
  gboolean writable = FALSE;
  g_assert_cmpstr(parent, ==, "wayland:window");
  g_assert_cmpstr(uri, ==, "https://example.com/path?x=1");
  g_assert_true(g_variant_lookup(options, "handle_token", "&s", &token));
  g_assert_cmpstr(token, ==, "host-token");
  g_assert_true(g_variant_lookup(options, "writable", "b", &writable));
  g_assert_true(writable);
  g_variant_unref(options);
  g_variant_unref(forwarded);
  g_variant_unref(parameters);
}

static void test_open_file_parameter_and_fd_forwarding(void) {
  int pipe_fds[2];
  g_assert_cmpint(pipe(pipe_fds), ==, 0);
  GError *error = NULL;
  GUnixFDList *source_fds = g_unix_fd_list_new();
  gint32 source_index = g_unix_fd_list_append(source_fds, pipe_fds[0], &error);
  g_assert_no_error(error);
  close(pipe_fds[0]);
  gint32 host_index = -1;
  GUnixFDList *host_fds =
      copy_open_uri_fd(source_fds, source_index, &host_index, &error);
  g_assert_no_error(error);
  g_assert_nonnull(host_fds);

  GVariant *parameters = g_variant_ref_sink(
      g_variant_new("(sh@a{sv})", "", source_index, test_options()));
  GVariant *forwarded = g_variant_ref_sink(open_uri_host_parameters(
      "OpenDirectory", parameters, "host-fd-token", host_index));
  const char *parent = NULL;
  gint32 forwarded_index = -1;
  GVariant *options = NULL;
  g_variant_get(forwarded, "(&sh@a{sv})", &parent, &forwarded_index, &options);
  g_assert_cmpstr(parent, ==, "");
  g_assert_cmpint(forwarded_index, ==, host_index);
  const char *token = NULL;
  g_assert_true(g_variant_lookup(options, "handle_token", "&s", &token));
  g_assert_cmpstr(token, ==, "host-fd-token");

  int forwarded_fd = g_unix_fd_list_get(host_fds, forwarded_index, &error);
  g_assert_no_error(error);
  const char sent = 'U';
  char received = '\0';
  g_assert_cmpint(write(pipe_fds[1], &sent, 1), ==, 1);
  g_assert_cmpint(read(forwarded_fd, &received, 1), ==, 1);
  g_assert_cmpint(received, ==, sent);

  close(forwarded_fd);
  close(pipe_fds[1]);
  g_variant_unref(options);
  g_variant_unref(forwarded);
  g_variant_unref(parameters);
  g_object_unref(host_fds);
  g_object_unref(source_fds);
}

static void test_missing_fd_list_is_rejected(void) {
  GError *error = NULL;
  gint32 index = -1;
  g_assert_null(copy_open_uri_fd(NULL, 0, &index, &error));
  g_assert_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT);
  g_clear_error(&error);
}

int main(void) {
  test_open_uri_introspection();
  test_open_uri_parameter_forwarding();
  test_open_file_parameter_and_fd_forwarding();
  test_missing_fd_list_is_rejected();
  return 0;
}
