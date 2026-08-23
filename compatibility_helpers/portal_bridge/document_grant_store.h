#ifndef DOCUMENT_GRANT_STORE_H
#define DOCUMENT_GRANT_STORE_H
#include "portal_bridge_process.h"
GVariant *path_bytes_variant(const char *);
void free_grant(DocumentGrant *);
void cleanup_grant(DocumentGrant *);
char **read_permissions(void);
void add_mountpoint_extra(BridgeState *, GVariantBuilder *);
bool sandbox_doc_dir_allowed(BridgeState *, const char *);
bool mount_grant_in_sandbox(DocumentGrant *, const char *, GError **);
void remove_sandbox_grants(BridgeState *, const char *);
bool create_document_grant_from_path(BridgeState *, const char *, const char *,
                                     char **, DocumentGrant **, GError **);
bool create_document_grant_from_fd(BridgeState *, int, const char *, GVariant *,
                                   DocumentGrant **, GError **);
DocumentGrant *find_grant(BridgeState *, const char *);
char *sandbox_uri_for_grant(BridgeState *, DocumentGrant *);
#endif
