async function b() { console.log("b1"); return 5; }
async function a() { console.log("a1"); var v = await b(); console.log("a2", v); }
a();
console.log("sync");
