import { Button } from "@/components/ui/button";

/** The slice of a react-query infinite query that the button needs. */
export interface PagedQuery {
  hasNextPage?: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => unknown;
}

/** The "Load more" button under a paged list: shown while another page exists, disabled while it loads. */
export function LoadMore({ query, label = "Load more", size }: { query: PagedQuery; label?: string; size?: "sm" }) {
  if (!query.hasNextPage) return null;
  return (
    <Button variant="outline" size={size} disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>
      {label}
    </Button>
  );
}
