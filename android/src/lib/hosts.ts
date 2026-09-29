/**
 * A colour for each computer this phone may read.
 *
 * The colour is worked out from the computer's own endpoint id rather than
 * stored anywhere: two phones looking at the same friend's computer draw it the
 * same colour, and nothing has to be kept in step when a computer is forgotten
 * and paired again.
 */

/** How many hues the wheel is divided into, so two computers rarely collide. */
const HUE_STEPS = 24;

/**
 * The hue one computer's marks are drawn in.
 *
 * A short hash of the endpoint id, rounded to a step on the wheel: rounding is
 * what keeps two nearby ids from drawing colours nobody can tell apart.
 */
export function hostHue(endpointId: string): number {
  let hash = 2166136261;
  for (let index = 0; index < endpointId.length; index += 1) {
    hash = Math.imul(hash ^ endpointId.charCodeAt(index), 16777619);
  }
  const step = (hash >>> 0) % HUE_STEPS;
  return Math.round((step * 360) / HUE_STEPS);
}

/** The colour itself, for a dot or a mark that is not styled from CSS. */
export function hostColour(endpointId: string): string {
  return `hsl(${hostHue(endpointId)} 72% 62%)`;
}
