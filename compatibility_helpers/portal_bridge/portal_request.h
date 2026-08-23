#ifndef PORTAL_REQUEST_H
#define PORTAL_REQUEST_H
#include "portal_bridge_process.h"
extern const char *REQUEST_XML;
extern const GDBusInterfaceVTable REQUEST_VTABLE;
void free_request(RequestRecord *);
RequestRecord *find_request(BridgeState *, const char *);
void emit_request_response(RequestRecord *, guint32, GVariant *);
void emit_cancel_response(RequestRecord *);
char *safe_path_element(const char *);
char *portal_path(const char *, const char *, const char *);
char *token_from_options(BridgeState *, GVariant *, const char *, const char *);
char *request_path_for_options(BridgeState *, const char *, GVariant *);
char *request_path_for_call(BridgeState *, const char *, GVariant *, gsize);
char *fresh_host_token(BridgeState *, const char *);
GVariant *rewrite_options(GVariant *, const char *, const char *);
#endif
