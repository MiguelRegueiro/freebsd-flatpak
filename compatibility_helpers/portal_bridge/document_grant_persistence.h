#ifndef DOCUMENT_GRANT_PERSISTENCE_H
#define DOCUMENT_GRANT_PERSISTENCE_H
#include "portal_bridge_process.h"
bool load_persistent_document_grants(BridgeState *, GError **);
bool save_persistent_document_grants(BridgeState *, GError **);
#endif
