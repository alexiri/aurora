---
name: grafana-opensearch
description: "Query OpenSearch or Elasticsearch data sources through Grafana with the grafana-os script, and the plugin's quirks"
---

# OpenSearch and Elasticsearch through Grafana

Use `grafana-query` for the UID, times, frames and errors; read a panel that queries the index
(`grafana-query panels <uid>`) for the real field names. `grafana-os` builds the plugin's Lucene query body
from flags (the Lucene query is one shell argument, never escaped by hand) and prints the answer like
`grafana-query run` (`--wide`, `--max-rows`, `--columns` are the same). Checked against
`grafana/opensearch-datasource` v2.34.7; Elasticsearch has the same model. **Seen on one instance** means
not checkable in the source: verify on the data.

```sh
O="$SKILLS_DIR/grafana-opensearch/scripts/grafana-os"
# counts and bytes per host, top 5 by bytes, 30-minute buckets, one table
"$O" --ds <uid> --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z \
  --query 'status:>=500 AND agent:"curl/8.0"' \
  --metric count --metric sum:bytes --terms host.keyword:5:sum:bytes --hist 30m --wide
# the three newest documents, three fields
"$O" --ds <uid> --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z \
  --query 'level:error' --raw 3 --columns clientip,path,status
```

- `--metric` (repeat, at most 6, default `count`): `count`, `sum:F`, `avg:F`, `max:F`, `min:F`,
  `cardinality:F`, `percentiles:F[:50,95,99]`. `--terms FIELD[:SIZE[:ORDER]]` (repeat, at most 3, nested in
  order; SIZE default 10; ORDER `_count`, `_key` or a metric spec such as `sum:bytes`). `--hist 30m|1h|1d`
  adds the innermost `date_histogram`.
- The time range and `--hist` use the data source's configured time field; `--time-field` only buckets on
  another field.
- `--raw N` (1 to 200): the newest N documents, not with aggregations. `--ds-type elasticsearch` for
  Elasticsearch. `--print-body` prints the body: put it in the report as the query that was run.
- For what flags cannot express (several refIds, `filters` or other buckets) write a body for
  `grafana-query run body.json`, such as `terms` under `date_histogram`:
  `"queryType":"lucene","query":"level:error","metrics":[{"id":"1","type":"count"}],"bucketAggs":[{"id":"2","type":"terms","field":"host.keyword","settings":{"size":"5","orderBy":"_count","order":"desc"}},{"id":"3","type":"date_histogram","field":"@timestamp","settings":{"interval":"1h"}}]`
  in a query with `datasource` `{"uid","type":"grafana-opensearch-datasource"}`.
  Documents: first metric `raw_data` (`settings.size`, `order: desc`; `raw_document` gives one JSON
  column), no `bucketAggs`. `logs` fails through `ds/query`: use `raw_data`.

## Quirks

- `queryType` is `lucene` (default) or `PPL`; any other value errors with `expected Lucene or PPL`: write
  lowercase `lucene`.
- A count needs a bucket aggregation: with none only `raw_data`/`raw_document` are valid, else `invalid
  query, missing metrics and aggregations`.
- A total over a window is the sum of every `date_histogram` bucket. For totals per term use `terms` alone.
- `terms` `orderBy` is `_count`, `_key`, or the `id` of one of the query's metrics; other numbers are
  ignored (default: count, descending). Ordering by a metric id came back unordered once, correct on later
  runs: check the order of the rows.
- `percentiles` under `terms` alone returns nulls (seen on one instance): add `--hist` as long as the window
  (`grafana-os` notes it on stderr when `--hist` is left out).
- Columns named just `Value` differ only by display name (`<term> Count`, `<term> Sum bytes`): the script
  uses it; `--wide` puts all series in one table.
- `cardinality` on an IP field returned 0 or null once: count `terms` buckets instead (`search.max_buckets`;
  default `terms` size 500). `sum` of a missing field returns 0, not an error: check the field in a panel.
- A large `terms` `size` with a fine `date_histogram` multiplies buckets: widen the interval or filter first.

## Reading log numbers

- Byte fields can exceed what crossed the network (aborted transfers log the full size): compare with the
  host's network metrics before reasoning about bandwidth.
- Check the unit of a duration or size on one raw document first (`--raw 1`): response times are often
  microseconds.
- A distinct count read from `terms` buckets is "at least N" when the buckets equal `size`: raise `size` or
  use `cardinality`.
