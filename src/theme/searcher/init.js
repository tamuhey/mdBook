import { search as tinysearch, default as init } from "/tinysearch_engine.js";
window.tinysearch = tinysearch;

async function run() {
    await init('/tinysearch_engine_bg.wasm');
}

run();