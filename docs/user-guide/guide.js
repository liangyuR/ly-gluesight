const viewer=document.getElementById('image-viewer');
document.querySelectorAll('figure img').forEach(img=>{
  img.tabIndex=0;
  img.setAttribute('role','button');
  img.setAttribute('aria-label','放大图片 '+img.alt);
  const open=()=>{viewer.querySelector('img').src=img.src;viewer.querySelector('img').alt=img.alt;viewer.querySelector('span').textContent=img.alt;viewer.showModal();};
  img.addEventListener('click',open);
  img.addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();open();}});
});
viewer.querySelector('button').addEventListener('click',()=>viewer.close());
viewer.addEventListener('click',event=>{if(event.target===viewer)viewer.close();});
document.getElementById('print-guide').addEventListener('click',()=>window.print());
