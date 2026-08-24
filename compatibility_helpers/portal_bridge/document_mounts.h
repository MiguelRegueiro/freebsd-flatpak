#ifndef DOCUMENT_MOUNTS_H
#define DOCUMENT_MOUNTS_H
#include "portal_bridge_process.h"
void cleanup_grant(DocumentGrant *);
bool sandbox_doc_dir_allowed(BridgeState *, const char *);
bool mount_grant_in_sandbox(DocumentGrant *, const char *, GError **);
void remove_sandbox_grants(BridgeState *, const char *);
#endif
