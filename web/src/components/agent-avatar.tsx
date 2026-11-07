/** Agent avatar: the public image when set (a same-origin path), otherwise an initial. */
export function AgentAvatar({
  name,
  url,
  size = 32,
}: {
  name: string;
  url?: string | null;
  size?: number;
}) {
  const style = { width: size, height: size };
  if (url) {
    // eslint-disable-next-line @next/next/no-img-element
    return <img src={url} alt="" width={size} height={size} style={style} className="shrink-0 rounded-full object-cover" />;
  }
  return (
    <span
      aria-hidden="true"
      style={{ ...style, fontSize: Math.round(size / 2.2) }}
      className="inline-flex shrink-0 items-center justify-center rounded-full bg-neutral-200 font-medium uppercase text-neutral-700"
    >
      {name.trim().charAt(0) || "?"}
    </span>
  );
}
