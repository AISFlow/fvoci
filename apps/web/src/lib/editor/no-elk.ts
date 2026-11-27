/**
 * Stands in for the ELK layout engine of the diagram library.
 *
 * The library can lay a diagram out with ELK, whose package (`elkjs`) is
 * under the EPL-2.0 or the GPL-3.0, neither of which the project's list of
 * licences allows. So the build puts this file in its place (the alias in
 * vite.config.ts), and diagrams are laid out with the library's other engine
 * (`layout: 'dagre'` in diagram.ts, which a diagram's own text cannot change).
 * Nothing calls this; if something ever does, it says why it cannot answer.
 */
export default class NoElk {
  layout(): Promise<never> {
    return Promise.reject(new Error('the ELK layout engine is not part of this build'));
  }
}
