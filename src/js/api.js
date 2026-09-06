// api.js — every backend call goes through here. One place to see the
// full surface of Rust commands the frontend depends on.
import { invoke } from '../include/tauri-api/core.js';

const Api = {
    // households
    searchHouseholds: (params) => invoke('search_households', { params }),
    getHousehold: (id) => invoke('get_household', { id }),
    updateHouseholdComments: (id, comments) => invoke('update_household_comments', { id, comments }),
    softDeleteHousehold: (id, reason) => invoke('soft_delete_household', { id, reason }),
    listDeletedHouseholds: () => invoke('list_deleted_households'),
    restoreDeletedHousehold: (id) => invoke('restore_deleted_household', { id }),

    // tags
    listTags: () => invoke('list_tags'),
    createTag: (name) => invoke('create_tag', { name }),
    renameTag: (id, newName) => invoke('rename_tag', { id, newName }),
    deleteTag: (id) => invoke('delete_tag', { id }),
    tagHouseholds: (householdIds, tagName) => invoke('tag_households', { householdIds, tagName }),
    untagHousehold: (householdId, tagId) => invoke('untag_household', { householdId, tagId }),
    bulkTagSearchResults: (search, tagName) => invoke('bulk_tag_search_results', { search, tagName }),

    // import
    importPdf: (filePath) => invoke('import_pdf', { filePath }),
    importCsv: (filePath) => invoke('import_csv', { filePath }),
    getReviewQueue: (batchId) => invoke('get_review_queue', { batchId }),
    resolveReviewItem: (itemId, action, comment) => invoke('resolve_review_item', { itemId, action, comment }),
    commitImportBatch: (batchId) => invoke('commit_import_batch', { batchId }),
    resolveAllNewRecords: (batchId) => invoke('resolve_all_new_records', { batchId }),
    getPendingImportBatch: () => invoke('get_pending_import_batch'),

    // visits / map
    recordVisit: (householdId, visitDate, comments) =>
        invoke('record_visit', { householdId, visitDate, comments }),
    getVisitsReport: (dateFrom, dateTo) => invoke('get_visits_report', { dateFrom, dateTo }),
    getHouseholdVisits: (householdId) => invoke('get_household_visits', { householdId }),
    generateVisitList: (params) => invoke('generate_visit_list', { params }),
    getMapData: (tagId) => invoke('get_map_data', { tagId }),

    // backup / restore
    backupDatabase: (destPath, passphrase) => invoke('backup_database', { destPath, passphrase }),
    restorePreview: (srcPath, passphrase) => invoke('restore_preview', { srcPath, passphrase }),
    restoreCommit: (srcPath, passphrase, token) => invoke('restore_commit', { srcPath, passphrase, token }),

    // roads
    ingestRoadDatabase: (filePath) => invoke('ingest_road_database', { filePath }),
    getRoadsInBounds: (minLat, maxLat, minLon, maxLon) =>
        invoke('get_roads_in_bounds', { minLat, maxLat, minLon, maxLon }),
    getNearestRoadNode: (lat, lon) => invoke('get_nearest_road_node', { lat, lon }),

    // settings / logs
    getSettings: () => invoke('get_settings'),
    saveSettings: (values) => invoke('save_settings', { values }),
    pruneOldDeletedAndLogs: () => invoke('prune_old_deleted_and_logs'),
    previewPruneImpact: (deletedDays, logDays) => invoke('preview_prune_impact', { deletedDays, logDays }),
    getLogs: (levels, page, pageSize) => invoke('get_logs', { levels, page, pageSize }),
};

window.Api = Api;
