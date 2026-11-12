// api.js — every backend call goes through here. One place to see the
// full surface of Rust commands the frontend depends on.
import { invoke } from '../include/tauri-api/core.js';

const Api = {
    // households
    searchHouseholds: (params) => invoke('search_households', { params }),
    getHousehold: (id) => invoke('get_household', { id }),
    updateHouseholdComments: (id, comments) => invoke('update_household_comments', { id, comments }),
    setContactPlan: (id, plan) => invoke('set_contact_plan', { id, plan }),
    softDeleteHousehold: (id, reason) => invoke('soft_delete_household', { id, reason }),
    listDeletedHouseholds: () => invoke('list_deleted_households'),
    restoreDeletedHousehold: (id) => invoke('restore_deleted_household', { id }),

    // tags
    listTags: () => invoke('list_tags'),
    createTag: (name) => invoke('create_tag', { name }),
    renameTag: (id, newName) => invoke('rename_tag', { id, newName }),
    deleteTag: (id) => invoke('delete_tag', { id }),
    tagHouseholds: (householdIds, tagName, allowSystemTagChange) =>
        invoke('tag_households', { householdIds, tagName, allowSystemTagChange }),
    untagHousehold: (householdId, tagId) => invoke('untag_household', { householdId, tagId }),
    bulkTagSearchResults: (search, tagName) => invoke('bulk_tag_search_results', { search, tagName }),

    // import
    importPdf: (filePath) => invoke('import_pdf', { filePath }),
    getReviewQueue: (batchId) => invoke('get_review_queue', { batchId }),
    resolveReviewItem: (itemId, action, comment, linkTargetId, confirmReferrals) =>
        invoke('resolve_review_item', { itemId, action, comment, linkTargetId, confirmReferrals: confirmReferrals || null }),
    commitImportBatch: (batchId) => invoke('commit_import_batch', { batchId }),
    resolveAllNewRecords: (batchId) => invoke('resolve_all_new_records', { batchId }),
    getPendingImportBatch: () => invoke('get_pending_import_batch'),
    discardImportBatch: (batchId) => invoke('discard_import_batch', { batchId }),

    // visits / map
    // kind: { method, outcome, invited }; omitted = a contacted visit.
    recordVisit: (householdId, visitDate, comments, kind = {}) =>
        invoke('record_visit', { householdId, visitDate, comments, method: kind.method ?? null, outcome: kind.outcome ?? null, invited: kind.invited ?? null }),
    updateVisit: (visitId, visitDate, comments, kind = {}) =>
        invoke('update_visit', { visitId, visitDate, comments, method: kind.method ?? null, outcome: kind.outcome ?? null, invited: kind.invited ?? null }),
    deleteVisit: (visitId) => invoke('delete_visit', { visitId }),
    getVisitsReport: (dateFrom, dateTo, tagId) => invoke('get_visits_report', { dateFrom, dateTo, tagId: tagId ?? null }),
    getCoverage: (dateFrom, dateTo, countAttempts) => invoke('get_coverage', { dateFrom, dateTo, countAttempts: !!countAttempts }),
    getContactPlans: () => invoke('get_contact_plans'),
    getHouseholdVisits: (householdId) => invoke('get_household_visits', { householdId }),
    getNeedsAttention: (tagId, thresholdDays, countAttempts) => invoke('get_needs_attention', { tagId, thresholdDays, countAttempts: !!countAttempts }),
    generateVisitList: (params) => invoke('generate_visit_list', { params }),
    getMapData: (tagId) => invoke('get_map_data', { tagId }),
    getMissingCoordsCount: () => invoke('get_missing_coords_count'),

    // clerk referrals
    listReferrals: (householdId) => invoke('list_referrals', { householdId }),
    createReferral: (householdId, kind, details, newAddress, learnedOn, visitUid) =>
        invoke('create_referral', { householdId, kind, details, newAddress, learnedOn, visitUid: visitUid || null }),
    updateReferral: (id, kind, details, newAddress, learnedOn) =>
        invoke('update_referral', { id, kind, details, newAddress, learnedOn }),
    markReferralsReported: (ids, reportedOn) => invoke('mark_referrals_reported', { ids, reportedOn }),
    closeReferral: (id, status, note) => invoke('close_referral', { id, status, note }),
    reopenReferral: (id) => invoke('reopen_referral', { id }),

    // backup / restore
    backupDatabase: (destPath, passphrase) => invoke('backup_database', { destPath, passphrase }),
    restorePreview: (srcPath, passphrase) => invoke('restore_preview', { srcPath, passphrase }),
    restoreCommit: (srcPath, passphrase, token) => invoke('restore_commit', { srcPath, passphrase, token }),

    // roads
    ingestRoadDatabase: (filePath) => invoke('ingest_road_database', { filePath }),
    getRoadsInBounds: (minLat, maxLat, minLon, maxLon) =>
        invoke('get_roads_in_bounds', { minLat, maxLat, minLon, maxLon }),
    getNearestRoadNode: (lat, lon) => invoke('get_nearest_road_node', { lat, lon }),

    // diagnostics
    findPotentialProblems: () => invoke('find_potential_problems'),

    // map tiles (#72)
    getCachedTile: (z, x, y) => invoke('get_cached_tile', { z, x, y }),
    saveCachedTile: (z, x, y, bytes) => invoke('save_cached_tile', { z, x, y, bytes }),
    getTileCacheStatus: () => invoke('get_tile_cache_status'),
    clearTileCache: () => invoke('clear_tile_cache'),

    // settings / logs
    getSettings: () => invoke('get_settings'),
    saveSettings: (values) => invoke('save_settings', { values }),
    pruneOldDeletedAndLogs: () => invoke('prune_old_deleted_and_logs'),
    previewPruneImpact: (deletedDays, logDays) => invoke('preview_prune_impact', { deletedDays, logDays }),
    listPruneCandidates: () => invoke('list_prune_candidates'),
    getLogs: (levels, page, pageSize) => invoke('get_logs', { levels, page, pageSize }),

    // profiles (#85)
    listProfiles: () => invoke('list_profiles'),
    getActiveProfile: () => invoke('get_active_profile'),
    createProfile: (name) => invoke('create_profile', { name }),
    switchProfile: (slug) => invoke('switch_profile', { slug }),
    restartApp: () => invoke('restart_app'),
};

window.Api = Api;
