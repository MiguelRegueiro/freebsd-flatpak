#ifndef SANDBOX_DOCUMENT_REGISTRATION_H
#define SANDBOX_DOCUMENT_REGISTRATION_H
#include "portal_bridge_process.h"
extern const char *CONTROL_XML;
extern const GDBusInterfaceVTable CONTROL_VTABLE;
bool add_sandbox(BridgeState *, const char *, GError **);
void remove_sandbox(BridgeState *, const char *);
#endif
