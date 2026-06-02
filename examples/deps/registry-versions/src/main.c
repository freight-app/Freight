#include <sqlite3.h>    /* registry dep: sqlite3   >=3.34.1 */
#include <zlib.h>       /* registry dep: zlib       1.3.1   */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Compress a string with zlib and report the ratio. */
static void compress_demo(const char *src) {
    uLong src_len = (uLong)strlen(src);
    uLong dst_len = compressBound(src_len);
    Bytef *dst = (Bytef *)malloc(dst_len);
    if (!dst) { fputs("malloc failed\n", stderr); return; }

    if (compress(dst, &dst_len, (const Bytef *)src, src_len) != Z_OK) {
        fputs("zlib compress failed\n", stderr);
        free(dst);
        return;
    }
    printf("zlib %s — compressed %lu → %lu bytes (%.1f%%)\n",
           zlibVersion(), src_len, dst_len,
           100.0 * (double)dst_len / (double)src_len);
    free(dst);
}

/* Open an in-memory SQLite database and run a quick query. */
static void sqlite_demo(void) {
    sqlite3 *db;
    if (sqlite3_open(":memory:", &db) != SQLITE_OK) {
        fprintf(stderr, "sqlite3_open: %s\n", sqlite3_errmsg(db));
        return;
    }

    const char *ddl =
        "CREATE TABLE pkg (name TEXT, version TEXT);"
        "INSERT INTO pkg VALUES ('zlib',   '1.3.1');"
        "INSERT INTO pkg VALUES ('sqlite3','3.34.1');"
        "INSERT INTO pkg VALUES ('fmt',    '10.2.0');";
    char *err = NULL;
    sqlite3_exec(db, ddl, NULL, NULL, &err);

    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "SELECT name, version FROM pkg ORDER BY name", -1, &stmt, NULL);
    printf("\nsqlite %s — packages in memory db:\n", sqlite3_libversion());
    while (sqlite3_step(stmt) == SQLITE_ROW) {
        printf("  %-20s %s\n",
               sqlite3_column_text(stmt, 0),
               sqlite3_column_text(stmt, 1));
    }
    sqlite3_finalize(stmt);
    sqlite3_close(db);
}

int main(void) {
    const char *payload =
        "freight registry example — zlib + sqlite3 resolved via local registry";

    compress_demo(payload);
    sqlite_demo();
    return 0;
}
