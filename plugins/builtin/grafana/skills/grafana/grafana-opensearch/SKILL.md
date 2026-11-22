---
name: grafana-opensearch
description: "Query OpenSearch or Elasticsearch data sources through Grafana: body format, aggregations and the plugin's quirks"
---

# OpenSearch and Elasticsearch through Grafana

Read `grafana-query` first (finding the UID, `from`/`to`, reading frames, errors). This skill is the
`grafana-os` script that builds the OpenSearch plugin's Lucene queries from flags, and the body format
behind it, sent with `query_datasources`
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

## The `grafana-os` script (preferred)

`grafana-os` builds the body from flags. The Lucene query is one shell argument and is never escaped by
hand inside JSON (the script puts it into the body with a JSON library). Use it first; it prints the
answer like `grafana-query run` (it calls that script, so `--wide`, `--max-rows` and `--columns` are
the same).

```sh
O="$SKILLS_DIR/grafana-opensearch/scripts/grafana-os"
# counts and bytes per host, top 5 by bytes, in 30-minute buckets, one table
"$O" --ds <uid> --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z \
  --query 'status:>=500 AND agent:"curl/8.0"' \
  --metric count --metric sum:bytes --terms host.keyword:5:sum:bytes --hist 30m --wide
# the three newest documents, three fields
"$O" --ds <uid> --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z \
  --query 'level:error' --raw 3 --columns clientip,path,status
```

- `--metric` (repeat, at most 6, default `count`): `count`, `sum:F`, `avg:F`, `max:F`, `min:F`,
  `cardinality:F`, `percentiles:F[:50,95,99]`. `--terms FIELD[:SIZE[:ORDER]]` (repeat, at most 3,
  nested in the order given; SIZE default 10; ORDER `_count`, `_key` or a `--metric` spec written the
  same way, `sum:bytes`). `--hist 30m|1h|1d` adds the innermost `date_histogram`.
- **Time field.** `--hist` uses the data source's configured time field when `--time-field` is not
  given: the plugin fills an empty `date_histogram` field with it (`lucene_handler.go`
  `addDateHistogramAgg`). Give `--time-field` only to bucket on another field. The `from`/`to` range
  always uses the configured field.
- `--raw N` (1 to 200) returns the newest N documents and cannot be combined with the aggregation
  flags. `--ds-type elasticsearch` for an Elasticsearch data source.
- `--print-body` prints the request body first: put it in the report as the query that was run.
- Fall back to `run body.json` for what `grafana-os` does not build: several queries (refIds) in one
  request, `filters` or other bucket aggregations, `raw_document`.

## A metrics query by hand: `terms` under `date_histogram`

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
`date_histogram` bucket and a `Value` column whose `labels` carry the term (`host.keyword=host-1`) and
whose display name is `host-1 Count`, as `grafana-query run` prints it:

```
# A 200
## frame host-1 Count
Time	host-1 Count
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
  order. A later test on the same instance (a `sum` metric, `orderBy: "1"`) sorted correctly, and
  ordering by a `sum` metric was honoured again on 2026-11-22, so check the order of the rows
  rather than assume either way.
- **`percentiles` under `terms` alone returns nulls** (seen on one instance, plugin 2.x). With the
  `date_histogram` as the innermost bucket it works: `grafana-os --metric percentiles:F --terms host --hist
  <the window's length>` gives one point per host. `grafana-os` prints a note on stderr when `--hist` is left
  out.
- **Frames whose field is just `Value`** differ only in `config.displayNameFromDS` (`<term> Count`,
  `<term> Sum bytes`, `<term> p95.0 latency`): the script names the columns with it, so the columns
  of one term can be told apart. With `--wide`, all series share one table.
- **`cardinality` on an IP field** returned 0 or null once, and worked on the same instance and
  field a day later. If it comes back 0 or null, count the buckets of a `terms` aggregation
  instead (mind `search.max_buckets`; the plugin's default `terms` size is 500).
- **`sum` of a missing field returns 0**, not an error (**seen on one instance**; OpenSearch's
  behaviour for a field that is not in the index): check the field name in a panel first.
- A `terms` with a large `size` and a fine `date_histogram` multiplies the buckets; widen the interval
  or filter with `query` first.

## Reading log numbers

- Byte fields in access logs can exceed what went over the network (aborted transfers are logged
  with their full size): compare with the host's network metrics before reasoning about bandwidth.
- Check the unit of a duration or size field on one raw document first (`grafana-os … --raw 1`): response
  times are often microseconds, not milliseconds.
- A distinct count read from `terms` buckets is "at least N" when the number of buckets equals
  `size`: raise `size`, or use a `cardinality` metric.
