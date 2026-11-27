/**
 * The files of the editor, rhwp-studio (the package `@fvoci/rhwp-studio`,
 * brought from rhwp's repository), as the frame loads them: each name is
 * turned into the file by an alias of the build (vite.config.ts). The
 * editor's source is not read by this project's type check, which these
 * names keep it out of.
 */

/** The editor's page (`index.html`), as text: main.ts puts its body into the frame's document. */
declare module 'rhwp-studio:page' {
  const page: string;
  export default page;
}

/** The editor's style sheet, the two scripts its page starts with, and its program: loaded for their effect. */
declare module 'rhwp-studio:style';
declare module 'rhwp-studio:theme-init';
declare module 'rhwp-studio:locale-init';
declare module 'rhwp-studio:main';
