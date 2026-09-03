#ifndef CAMERA_PORTAL_H
#define CAMERA_PORTAL_H
#include "portal_bridge_process.h"
bool camera_sender_is_allowed(BridgeState *, const char *);
void camera_portal_allow_sender(BridgeState *, const char *);
void camera_portal_forget_sender(BridgeState *, const char *);
void camera_portal_apply_response(RequestRecord *, guint32);
void handle_camera_method(BridgeState *, const char *, const char *, GVariant *,
                          GDBusMethodInvocation *);
#endif
