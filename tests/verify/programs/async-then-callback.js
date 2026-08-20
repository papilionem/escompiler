async function b() { return 5; }
b().then(function (v) { console.log("got", v); });
console.log("sync");
