window.wallpaperRegisterAudioListener = function(callback) {
  window._irisAudioListener = callback;
};
window.wallpaperPropertyListener = {
  applyUserProperties: function(props) {},
  applyGeneralProperties: function(props) {}
};
window.iris = {
  version: "0.1.0",
  postMessage: function(msg) { window.ipc.postMessage(msg); }
};
