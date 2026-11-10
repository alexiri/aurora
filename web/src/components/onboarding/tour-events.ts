/** The tour lives in the app shell (it points at the real navigation); anything can ask it to start. */
export const TOUR_EVENT = "aurora:tour";

export function startTour(): void {
  window.dispatchEvent(new Event(TOUR_EVENT));
}
