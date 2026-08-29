#include "sandbox_document_registration.h"
#include "document_mounts.h"
#include "portal_bridge_process.h"
const char *CONTROL_XML =
    "<node>"
    "  <interface name='org.freebsd.Flatpak.PortalBridge'>"
    "    <method name='AddSandbox'>"
    "      <arg type='s' name='sandbox_doc_dir' direction='in'/>"
    "    </method>"
    "    <method name='RemoveSandbox'>"
    "      <arg type='s' name='sandbox_doc_dir' direction='in'/>"
    "    </method>"
    "  </interface>"
    "</node>";

gint find_sandbox_doc_dir(BridgeState *state, const char *path) {
  for (guint i = 0; i < state->documents.sandbox_doc_dirs->len; i++) {
    if (g_strcmp0(g_ptr_array_index(state->documents.sandbox_doc_dirs, i),
                  path) == 0) {
      return (gint)i;
    }
  }
  return -1;
}

bool add_sandbox(BridgeState *state, const char *sandbox_doc_dir,
                 GError **error) {
  if (!sandbox_doc_dir_allowed(state, sandbox_doc_dir)) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED,
                "sandbox document directory is outside %s",
                state->documents.sandbox_root);
    return false;
  }
  if (find_sandbox_doc_dir(state, sandbox_doc_dir) >= 0) {
    return true;
  }
  for (guint i = 0; i < state->documents.grants->len; i++) {
    if (!mount_grant_in_sandbox(g_ptr_array_index(state->documents.grants, i),
                                sandbox_doc_dir, error)) {
      remove_sandbox_grants(state, sandbox_doc_dir, NULL);
      return false;
    }
  }
  g_ptr_array_add(state->documents.sandbox_doc_dirs,
                  g_strdup(sandbox_doc_dir));
  diagnostic_line("attached sandbox document root %s", sandbox_doc_dir);
  return true;
}

bool remove_sandbox(BridgeState *state, const char *sandbox_doc_dir,
                    GError **error) {
  gint index = find_sandbox_doc_dir(state, sandbox_doc_dir);
  if (index < 0) {
    return true;
  }
  if (!remove_sandbox_grants(state, sandbox_doc_dir, error)) {
    return false;
  }
  g_ptr_array_remove_index(state->documents.sandbox_doc_dirs, (guint)index);
  diagnostic_line("detached sandbox document root %s", sandbox_doc_dir);
  return true;
}

void handle_control_method(GDBusConnection *connection, const gchar *sender,
                           const gchar *object_path,
                           const gchar *interface_name,
                           const gchar *method_name, GVariant *parameters,
                           GDBusMethodInvocation *invocation,
                           gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  BridgeState *state = user_data;
  const char *sandbox_doc_dir = NULL;
  g_variant_get(parameters, "(&s)", &sandbox_doc_dir);
  if (g_strcmp0(method_name, "AddSandbox") == 0) {
    GError *error = NULL;
    if (!add_sandbox(state, sandbox_doc_dir, &error)) {
      g_dbus_method_invocation_take_error(invocation, error);
      return;
    }
    g_dbus_method_invocation_return_value(invocation, NULL);
    return;
  }
  if (g_strcmp0(method_name, "RemoveSandbox") == 0) {
    GError *error = NULL;
    if (!remove_sandbox(state, sandbox_doc_dir, &error)) {
      g_dbus_method_invocation_take_error(invocation, error);
      return;
    }
    g_dbus_method_invocation_return_value(invocation, NULL);
    return;
  }
  g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                        G_IO_ERROR_NOT_SUPPORTED,
                                        "%s is not implemented", method_name);
}

const GDBusInterfaceVTable CONTROL_VTABLE = {
    .method_call = handle_control_method,
};
