---
name: grafana-opensearch
description: "Query OpenSearch or Elasticsearch data sources through Grafana: body format, aggregations and the plugin's quirks"
---

# OpenSearch and Elasticsearch through Grafana

Read `grafana-query` first (finding the UID, `from`/`to`, reading frames, errors). This skill is the
body format of the OpenSearch plugin's Lucene queries, sent with `query_datasources`
(`"$SKILLS_DIR/grafana-query/scripts/grafana-query" run body.json`). Checked against the plugin
`grafana/opensearch-datasource` v2.34.7 (`pkg/opensearch/`: `models.go`, `query_request.go`,
`lucene_handler.go`, `response_parser.go`). The Elasticsearch plugin has the same query model.
Items marked **seen on one instance** could not be checked in the source: treat them as hints and
verify on the data.

## Which fields to use

Read a dashboard panel that queries the index first (`grafana-query panels <uid>`): its `targets[]`
show the real field names (`metrics[].field`, `bucketAggs[].field`) and the `query` syntax. The
plugin uses the **data source's configured time field** for the time range and for the default
`date_histogram`; a `timeField` in the query is not used (`query_request.go`).

## A metrics query: `terms` under `date_histogram`

```json
{"from": "now-12h", "to": "now",
 "queries": [{"refId": "A",
   "datasource": {"uid": "<uid>", "type": "grafana-opensearch-datasource"},
   "queryType": "lucene", "query": "level:error",
   "metrics": [{"id": "1", "type": "count"}],
   "bucketAggs": [
     {"id": "2", "type": "terms", "field": "host.keyword",
      "settings": {"size": "5", "orderBy": "_count", "order": "desc"}},
     {"id": "3", "type": "date_histogram", "field": "@timestamp", "settings": {"interval": "1h"}}],
   "intervalMs": 3600000, "maxDataPoints": 100}]}
```

The answer has one frame per `terms` bucket (the top five hosts): a `Time` column with one row per
`date_histogram` bucket and a `Value` column whose `labels` carry the term (`host.keyword=host-1`), as
`grafana-query run` prints:

```
# A 200
## frame host-1 Count
Time	Value{host.keyword=host-1}
2026-11-20T13:00:00.000Z	120
```

## Raw documents

- A query whose **first metric** has type `raw_data` or `raw_document` returns documents, with no
  `bucketAggs`: `"metrics": [{"id": "1", "type": "raw_data", "settings": {"size": "50", "order": "desc"}}]`
  (`lucene_handler.go`: such a query is processed as a document query, newest first with
  `order: "desc"`, `size` default 500). The time range still applies. Each document is a row of the frame, one column per field.
  `raw_document` is the same, but each row is a single JSON column holding the whole document.
  Do not use `logs` through `ds/query`: Grafana's Explore sends it as a separate logs request, and
  on the instances tested (plugin 2.x) a `logs` metric fails with `invalid query, missing metrics
  and aggregations`. Use `raw_data` instead.
- Keep `size` small (50 to 200).
- `raw_data` returns every field of the document as a column (30 or more on a typical access-log
  index), and the plugin offers no field selection. Keep `size` small and pick columns when
  printing: `grafana-query run body.json --columns clientip,path,status` (see `grafana-query`).

## Quirks

- **`queryType` casing.** The values are `lucene` (the default when the field is missing) and `PPL`
  (`models.go`). The error for any other value reads `invalid queryType: "…", expected Lucene or PPL`
  with a capital L (`query_request.go`), which is misleading: write `lucene` in lowercase.
- **A count needs a bucket aggregation.** With no `bucketAggs`, only `raw_data`, `raw_document` and
  `logs` metrics are valid in the plugin source; anything else fails with `invalid query, missing
  metrics and aggregations` (`lucene_handler.go` `processQuery`). `logs` fails the same way through
  `ds/query` (see Raw documents), so use `raw_data` for documents.
- **A total over a window is the sum of every `date_histogram` bucket**: a count with a
  `date_histogram` returns one point per bucket, not one number. A 12 h window with `interval: 12h`
  gives two buckets; add the values of every row. For totals per term, group by `terms` alone
  (no `date_histogram`): the answer is one row per term.
- **`orderBy` of a `terms` aggregation.** Use `_count` (or `_key`/`_term`). A numeric `orderBy` is
  only honoured when it is the `id` of one of the query's metrics (a `count` metric maps to
  `_count`); a number matching no metric sends no ordering, and OpenSearch's default (document
  count, descending) applies (`addTermsAgg`). Seen once: an `orderBy` by metric id came back in term
  order. A later test on the same instance (a `sum` metric, `orderBy: "1"`) sorted correctly, so
  check the order of the rows rather than assume either way.
- **`cardinality` on an IP field** returned 0 or null once, and worked on the same instance and
  field a day later. If it comes back 0 or null, count the buckets of a `terms` aggregation
  instead (mind `search.max_buckets`; the plugin's default `terms` size is 500).
- **`sum` of a missing field returns 0**, not an error (**seen on one instance**; OpenSearch's
  behaviour for a field that is not in the index): check the field name in a panel first.
- A `terms` with a large `size` and a fine `date_histogram` multiplies the buckets; widen the interval
  or filter with `query` first.
