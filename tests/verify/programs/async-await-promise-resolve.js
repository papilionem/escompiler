async function a() { console.log("a1"); var v = await Promise.resolve(5); console.log("a2", v); }
a();
console.log("sync");
