function* g(){ yield 1; yield 2; }
for (const v of g()) console.log(v);
