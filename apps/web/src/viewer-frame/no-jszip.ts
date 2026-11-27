/**
 * Stands in for the ZIP library (JSZip) where the PowerPoint renderer
 * imports it (tools/build/pptx-renderer.ts): the renderer uses it only to
 * read a whole archive (`parseZip`), and the viewer hands it the parts the
 * PowerPoint worker read instead. The Word renderer's JSZip is the real one.
 */
const noZip = {
  loadAsync(): never {
    throw new Error('archives are read by the PowerPoint worker in this build');
  },
};

export default noZip;
