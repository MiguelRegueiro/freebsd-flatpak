#ifndef SCREENCAST_PORTAL_H
#define SCREENCAST_PORTAL_H
#include "portal_bridge_process.h"
extern const char *SESSION_XML;
extern const GDBusInterfaceVTable SESSION_VTABLE;
void free_session(SessionRecord *);
SessionRecord *find_session(BridgeState *, const char *);
void close_host_session(SessionRecord *);
void update_session_sources(SessionRecord *, GVariant *);
void handle_screencast_create(BridgeState *, const char *, GVariant *,
                              GDBusMethodInvocation *);
void handle_screencast_request(BridgeState *, const char *, const char *,
                               GVariant *, GDBusMethodInvocation *);
gint32 copy_unix_fd(GUnixFDList *, gint32, GUnixFDList *, GError **);
void handle_open_pipewire_remote(BridgeState *, const char *, GVariant *,
                                 GDBusMethodInvocation *);
#endif
