//// [a.ts]
Object.defineProperty(exports, "__esModule", {
    value: !0
}), Object.defineProperty(exports, "default", {
    enumerable: !0,
    get: function() {
        return _default;
    }
});
const x = new Promise((resolve, reject)=>{
    resolve({});
}), _default = x;
//// [b.ts]
Object.defineProperty(exports, "__esModule", {
    value: !0
});
const _async_to_generator = require("@swc/helpers/_/_async_to_generator"), _interop_require_default = require("@swc/helpers/_/_interop_require_default"), _a = /*#__PURE__*/ _interop_require_default._(require("./a"));
_async_to_generator._(function*() {
    yield _a.default;
})();
