/** Reference night city for the grader's self-test. */
export function createScene(THREE) {
  const scene = new THREE.Scene();
  scene.background = new THREE.Color(0x05070f);
  scene.fog = new THREE.Fog(0x05070f, 30, 120);
  const camera = new THREE.PerspectiveCamera(55, 16 / 9, 0.1, 400);
  const ground = new THREE.Mesh(new THREE.PlaneGeometry(200, 200), new THREE.MeshStandardMaterial({ color: 0x111318 }));
  ground.rotation.x = -Math.PI / 2;
  scene.add(ground);
  // Lit windows: a small grid texture, some cells lit, used as the emissive map.
  const data = new Uint8Array(8 * 16 * 4);
  for (let i = 0; i < 8 * 16; i++) {
    const lit = (i * 37) % 5 < 2;
    data.set(lit ? [255, 214, 140, 255] : [10, 12, 18, 255], i * 4);
  }
  const grid = new THREE.DataTexture(data, 8, 16);
  grid.magFilter = THREE.NearestFilter;
  grid.needsUpdate = true;
  const windows = new THREE.MeshStandardMaterial({ color: 0x1a1d24, emissive: 0xffffff, emissiveMap: grid, emissiveIntensity: 1.2 });
  for (let x = -5; x <= 5; x++) {
    for (let z = -5; z <= 5; z++) {
      if ((x + z) % 3 === 0) continue;
      const height = 4 + ((x * 7 + z * 13 + 50) % 17) * 1.5;
      const building = new THREE.Mesh(new THREE.BoxGeometry(4, height, 4), windows);
      building.position.set(x * 8, height / 2, z * 8);
      building.userData.kind = "building";
      scene.add(building);
    }
  }
  scene.add(new THREE.AmbientLight(0x334466, 0.6));
  const moon = new THREE.DirectionalLight(0xaabbff, 1.2);
  moon.position.set(-40, 80, 20);
  scene.add(moon);
  const update = t => {
    const angle = t * 0.1;
    camera.position.set(Math.cos(angle) * 70, 35, Math.sin(angle) * 70);
    camera.lookAt(0, 8, 0);
  };
  update(0);
  return { scene, camera, update };
}
