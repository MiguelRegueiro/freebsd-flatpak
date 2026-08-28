#ifndef OPEN_URI_PORTAL_H
#define OPEN_URI_PORTAL_H
#include "portal_bridge_process.h"
void handle_open_uri_request(BridgeState *, const char *, const char *,
                             GVariant *, GDBusMethodInvocation *);
GVariant *open_uri_host_parameters(const char *, GVariant *, const char *,
                                   gint32);
GUnixFDList *copy_open_uri_fd(GUnixFDList *, gint32, gint32 *, GError **);
#endif
