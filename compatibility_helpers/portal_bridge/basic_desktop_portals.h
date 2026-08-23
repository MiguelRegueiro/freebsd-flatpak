#ifndef BASIC_DESKTOP_PORTALS_H
#define BASIC_DESKTOP_PORTALS_H
#include "portal_bridge_process.h"
extern const char *DESKTOP_XML;
extern const GDBusInterfaceVTable DESKTOP_VTABLE;
void on_forward_call(GObject *, GAsyncResult *, gpointer);
GVariant *handle_get_property(GDBusConnection *, const gchar *, const gchar *,
                              const gchar *, const gchar *, GError **,
                              gpointer);
#endif
