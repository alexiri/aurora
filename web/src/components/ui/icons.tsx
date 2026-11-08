import type { ReactNode, SVGProps } from "react";

/**
 * Inline stroke icons (16 px grid scaled by CSS). One file, no icon library.
 * Decorative by default (aria-hidden); pass aria-label on the parent control.
 */
function make(children: ReactNode) {
  return function Icon(props: SVGProps<SVGSVGElement>) {
    return (
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={1.75}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        focusable="false"
        width={16}
        height={16}
        {...props}
      >
        {children}
      </svg>
    );
  };
}

export const HomeIcon = make(<><path d="M3 11l9-8 9 8"/><path d="M5 10v10h14V10"/><path d="M10 20v-6h4v6"/></>);
export const BotIcon = make(<><rect x="4" y="8" width="16" height="12" rx="3"/><path d="M12 8V4"/><circle cx="12" cy="3.5" r="1"/><path d="M9 14h.01M15 14h.01"/></>);
export const KeyIcon = make(<><circle cx="8" cy="15" r="4"/><path d="M11 12l9-9"/><path d="M16 7l3 3"/></>);
export const ShieldIcon = make(<><path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z"/></>);
export const CheckCircleIcon = make(<><circle cx="12" cy="12" r="9"/><path d="M8 12l3 3 5-6"/></>);
export const ActivityIcon = make(<><path d="M3 12h4l3-8 4 16 3-8h4"/></>);
export const SettingsIcon = make(<><circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/></>);
export const UsersIcon = make(<><circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c0-3.6 2.9-6 6.5-6s6.5 2.4 6.5 6"/><path d="M16 4.6a3.5 3.5 0 010 6.8M18 14.3c2.2.7 3.5 2.6 3.5 5.2"/></>);
export const BuildingIcon = make(<><rect x="5" y="3" width="14" height="18" rx="1"/><path d="M9 7h2M13 7h2M9 11h2M13 11h2M9 15h2M13 15h2"/></>);
export const PlugIcon = make(<><path d="M9 3v5M15 3v5"/><path d="M6 8h12v3a6 6 0 01-12 0z"/><path d="M12 17v4"/></>);
export const ScaleIcon = make(<><path d="M12 3v18M5 21h14"/><path d="M5 7h14"/><path d="M5 7l-3 7a3.5 3.5 0 006 0zM19 7l-3 7a3.5 3.5 0 006 0z"/></>);
export const BookIcon = make(<><path d="M4 4.5A1.5 1.5 0 015.5 3H20v16H5.5A1.5 1.5 0 004 20.5z"/><path d="M4 20.5A1.5 1.5 0 005.5 22H20v-3"/></>);
export const MessageIcon = make(<><path d="M4 5h16v11H9l-5 4z"/></>);
export const ScrollIcon = make(<><path d="M8 3h11v15a3 3 0 01-3 3H6a3 3 0 003-3V6a3 3 0 00-3-3"/><path d="M12 8h4M12 12h4"/></>);
export const UserIcon = make(<><circle cx="12" cy="8" r="4"/><path d="M4 21c0-4 3.6-6.5 8-6.5s8 2.5 8 6.5"/></>);
export const LockIcon = make(<><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 018 0v4"/></>);
export const TrashIcon = make(<><path d="M4 7h16M10 3h4M6 7l1 14h10l1-14M10 11v6M14 11v6"/></>);
export const ServerIcon = make(<><rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="13" width="18" height="7" rx="2"/><path d="M7 7.5h.01M7 16.5h.01"/></>);
export const SunIcon = make(<><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></>);
export const MoonIcon = make(<><path d="M20 14.5A8 8 0 019.5 4 8 8 0 1020 14.5z"/></>);
export const MonitorIcon = make(<><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 21h8M12 16v5"/></>);
export const ChevronsUpDownIcon = make(<><path d="M7 9l5-5 5 5M7 15l5 5 5-5"/></>);
export const ChevronRightIcon = make(<><path d="M9 6l6 6-6 6"/></>);
export const MenuIcon = make(<><path d="M4 6h16M4 12h16M4 18h16"/></>);
export const XIcon = make(<><path d="M6 6l12 12M18 6L6 18"/></>);
export const PlusIcon = make(<><path d="M12 5v14M5 12h14"/></>);
export const PaperclipIcon = make(<><path d="M20 11.5l-8 8a5 5 0 01-7-7l9-9a3.5 3.5 0 015 5l-9 9a2 2 0 01-3-3l8-8"/></>);
export const SendIcon = make(<><path d="M21 3L10 14M21 3l-7 18-4-7-7-4z"/></>);
export const ClockIcon = make(<><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></>);
export const PlugsIcon = make(<><path d="M4 12h6M14 12h6M10 8v8M14 8v8"/></>);
export const LinkIcon = make(<><path d="M10 14a4 4 0 005.7 0l3-3a4 4 0 00-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 00-5.7 0l-3 3A4 4 0 0011 18.7l1-1"/></>);
export const CheckIcon = make(<><path d="M5 12l5 5 9-10"/></>);
export const WrenchIcon = make(<><path d="M14.5 6.5a4 4 0 005 5L20 12l-8 8a2.1 2.1 0 01-3-3l8-8z"/><path d="M14.5 6.5l3 3"/></>);
export const ChevronDownIcon = make(<><path d="M6 9l6 6 6-6"/></>);
export const DownloadIcon = make(<><path d="M12 4v11M7 11l5 5 5-5M5 20h14"/></>);
export const FileIcon = make(<><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/></>);
export const EyeIcon = make(<><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></>);
export const MoreIcon = make(<><path d="M5 12h.01M12 12h.01M19 12h.01"/></>);
export const CopyIcon = make(<><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 012-2h9"/></>);
export const ArrowLeftIcon = make(<><path d="M19 12H5M11 6l-6 6 6 6"/></>);
export const SearchIcon = make(<><circle cx="11" cy="11" r="7"/><path d="M20 20l-4-4"/></>);
export const StopIcon = make(<><rect x="6" y="6" width="12" height="12" rx="2"/></>);
export const CalendarIcon = make(<><rect x="4" y="5" width="16" height="16" rx="2"/><path d="M4 10h16M9 3v4M15 3v4"/></>);
